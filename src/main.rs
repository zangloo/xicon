use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::SystemTime;
use anyhow::{anyhow, Result};
use clap::Parser;
use fork::Fork;
use regex::Regex;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{Atom, AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConfigureWindowAux, ConnectionExt, EventMask, PropMode, Screen, Window};
use x11rb::rust_connection::RustConnection;

#[derive(Clone, Debug)]
enum WindowMatchProperty {
	Class(String),
	Name(String),
}

impl<'a> From<&'a str> for WindowMatchProperty {
	fn from(value: &'a str) -> Self
	{
		let re = Regex::new(r"^((class)|(name))=(.+)$").unwrap();
		let captures = re.captures(value)
			.unwrap_or_else(|| panic!("Invalid match property: {value}"));
		if let (Some(type_), Some(name)) = (captures.get(1), captures.get(4)) {
			if type_.as_str() == "class" {
				WindowMatchProperty::Class(name.as_str().to_owned())
			} else {
				WindowMatchProperty::Name(name.as_str().to_owned())
			}
		} else {
			panic!("Invalid match property: {value}")
		}
	}
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum WindowSize {
	Max,
	Min,
	Fullscreen,
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum WindowType {
	Desktop,
	Dock,
	Toolbar,
	Menu,
	Utility,
	Splash,
	Dialog,
	Normal,
}

struct WindowGeometry {
	size: Option<(u32, u32)>,
	offset: Option<(bool, i32, bool, i32)>,
}

impl WindowType {
	fn as_str(&self) -> &'static str
	{
		match self {
			WindowType::Desktop => "_NET_WM_WINDOW_TYPE_DESKTOP",
			WindowType::Dock => "_NET_WM_WINDOW_TYPE_DOCK",
			WindowType::Toolbar => "_NET_WM_WINDOW_TYPE_TOOLBAR",
			WindowType::Menu => "_NET_WM_WINDOW_TYPE_MENU",
			WindowType::Utility => "_NET_WM_WINDOW_TYPE_UTILITY",
			WindowType::Splash => "_NET_WM_WINDOW_TYPE_SPLASH",
			WindowType::Dialog => "_NET_WM_WINDOW_TYPE_DIALOG",
			WindowType::Normal => "_NET_WM_WINDOW_TYPE_NORMAL",
		}
	}
}

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Cli {
	#[clap(short, long, help = "window match property, <class|name>=<property value>")]
	property: Option<WindowMatchProperty>,
	#[clap(short, long, help = "icon file")]
	icon: Option<PathBuf>,
	#[clap(short, long, value_enum)]
	size: Option<WindowSize>,
	#[clap(short, long, help = "always on top")]
	above: bool,
	#[clap(short = 'd', long, help = "no decoration")]
	no_decoration: bool,
	#[clap(short = 't', long = "type")]
	win_type: Option<WindowType>,
	#[clap(short, long, help = "format: [<width>{xX}<height>][{+-}<xoffset>{+-}<yoffset>]", allow_hyphen_values = true)]
	geometry: Option<String>,
	#[clap(short = 'k', long, help = "hide window in taskbar")]
	no_taskbar_icon: bool,
	#[clap(short, long, default_value = "10", help = "max seconds to wait for program to complete startup")]
	wait: u64,
	#[clap(short, long, help = "x11 program to run")]
	command: String,
	args: Vec<String>,
}

fn main() -> Result<()>
{
	let cli = Cli::parse();
	if let Some(icon) = &cli.icon {
		if !icon.exists() {
			panic!("Icon file not exists: {:#?}", cli.icon)
		}
	}

	match fork::daemon(false, true) {
		Ok(Fork::Parent(_)) => Ok(()),
		Ok(Fork::Child) => start(cli),
		Err(_) => Err(anyhow!("Failed fork")),
	}
	// start(cli)
}

struct IconData {
	data: Vec<u8>,
	length: u32,
}

#[inline]
fn start(cli: Cli) -> Result<()>
{
	let (conn, screen_num) = x11rb::connect(None)?;
	let screen = &conn.setup().roots[screen_num];
	let state_atom = get_atom(&conn, "_NET_WM_STATE")?;

	let mut aux = ChangeWindowAttributesAux::new();
	aux.event_mask = Some(EventMask::SUBSTRUCTURE_NOTIFY);
	conn.change_window_attributes(screen.root, &aux)?.check()?;
	conn.flush()?;
	let child = Command::new(cli.command).args(cli.args).spawn()?;
	let pid = child.id();
	let start = SystemTime::now();
	loop {
		let event = conn.wait_for_event()?;
		if let Event::ReparentNotify(event) = event {
			let win = event.window;
			if match_window(&conn, win, pid, &cli.property)? {
				if let Some(icon) = &cli.icon {
					let icon = load_icon(icon)?;
					set_icon(&conn, win, &icon)?;
				}
				if let Some(size) = &cli.size {
					set_size(&conn, screen.root, win, size, state_atom)?;
				}
				if cli.above {
					set_above(&conn, screen.root, win, state_atom)?;
				}
				if cli.no_decoration {
					remove_decoration(&conn, win)?;
				}
				if let Some(win_type) = &cli.win_type {
					set_type(&conn, win, win_type)?;
				}
				if let Some(geometry) = &cli.geometry {
					set_geometry(&conn, screen, win, geometry)?;
				}
				if cli.no_taskbar_icon {
					hide_taskbar_icon(&conn, screen.root, win, state_atom)?;
				}
				break;
			}
		}
		let now = SystemTime::now();
		let duration = now.duration_since(start)
			.expect("Clock may have gone backwards");
		if duration.as_secs() > cli.wait {
			eprintln!("Failed to detect command windows in {} seconds, quit.", cli.wait);
			break;
		}
	}
	Ok(())
}

fn match_window(conn: &RustConnection, current: Window, target_pid: u32,
	match_property: &Option<WindowMatchProperty>) -> Result<bool>
{
	match match_property {
		None => {
			let pid_atom = get_atom(&conn, "_NET_WM_PID")?;
			let pid_result = conn.get_property(
				false,
				current,
				pid_atom,
				AtomEnum::CARDINAL,
				0, 1,
			)?;
			let pid_reply = pid_result.reply()?;
			if pid_reply.length == 1 {
				let pid = pid_reply.value32()
					.expect("Invalid replay")
					.next()
					.expect("No pid exists in result");
				Ok(pid == target_pid)
			} else {
				Ok(false)
			}
		}
		Some(WindowMatchProperty::Class(value)) => {
			let len = value.len();
			let result = conn.get_property(
				false,
				current,
				AtomEnum::WM_CLASS,
				AtomEnum::STRING,
				0,
				len as u32)?;
			let reply = result.reply()?;
			let win_value = reply.value;
			// class with two null-separated strings
			let bytes = value.as_bytes();
			for buf in win_value.split(|b| *b == 0) {
				if buf.len() == len {
					if compare_bytes(buf, bytes, len) {
						return Ok(true);
					}
				}
			}
			Ok(false)
		}
		Some(WindowMatchProperty::Name(value)) => {
			let len = value.len();
			let result = conn.get_property(
				false,
				current,
				AtomEnum::WM_NAME,
				AtomEnum::STRING,
				0,
				len as u32)?;
			let reply = result.reply()?;
			let win_value = reply.value;
			if win_value.len() == len {
				Ok(compare_bytes(&win_value, value.as_bytes(), len))
			} else {
				Ok(false)
			}
		}
	}
}

#[inline]
fn compare_bytes(a: &[u8], b: &[u8], len: usize) -> bool
{
	for i in 0..len {
		if a[i] != b[i] {
			return false;
		}
	}
	true
}

#[inline]
fn push_u32(data: &mut Vec<u8>, value: u32)
{
	let bytes = value.to_le_bytes();
	for byte in bytes {
		data.push(byte);
	}
}

fn load_icon(icon: &PathBuf) -> Result<IconData>
{
	let data = fs::read(icon)?;
	let image = image::load_from_memory(&data)?;
	let width = image.width();
	let height = image.height();
	let bytes = image.into_bytes();
	let mut data = vec![];
	push_u32(&mut data, width);
	push_u32(&mut data, height);
	let mut slice = bytes.as_slice();
	while let [r, g, b, a, rest @ ..] = slice {
		data.push(*b);
		data.push(*g);
		data.push(*r);
		data.push(*a);
		slice = rest;
	}
	let length = width * height + 2;
	Ok(IconData { data, length })
}

#[inline]
fn set_icon(conn: &RustConnection, win: Window, icon: &IconData) -> Result<()>
{
	let set_icon_atom = get_atom(&conn, "_NET_WM_ICON")?;
	conn.change_property(
		PropMode::REPLACE,
		win,
		set_icon_atom,
		AtomEnum::CARDINAL,
		32,
		icon.length,
		&icon.data,
	)?.check()?;
	Ok(())
}

#[inline]
fn send_message(conn: &RustConnection, root: Window, win: Window,
	msg_type: Atom, data: [u32; 5]) -> Result<()>
{
	let event = ClientMessageEvent::new(
		32, win, msg_type, data);

	conn.send_event(
		true,
		root,
		EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
		event,
	)?.check()?;
	Ok(())
}

#[inline]
fn set_size(conn: &RustConnection, root: Window, win: Window,
	size: &WindowSize, state_atom: Atom) -> Result<()>
{
	match size {
		WindowSize::Max => {
			let vertical = get_atom(conn, "_NET_WM_STATE_MAXIMIZED_VERT")?;
			let horizontal = get_atom(conn, "_NET_WM_STATE_MAXIMIZED_HORZ")?;
			add_state(conn, root, win, state_atom, vertical, horizontal, 1, 0)?;
		}
		WindowSize::Min => {
			let atom = get_atom(conn, "_NET_WM_STATE_HIDDEN")?;
			add_state(conn, root, win, state_atom, atom, 0, 0, 0)?;
		}
		WindowSize::Fullscreen => {
			let fs = get_atom(conn, "_NET_WM_STATE_FULLSCREEN")?;
			add_state(conn, root, win, state_atom, fs, 0, 0, 0)?;
		}
	}
	Ok(())
}

#[inline]
fn set_above(conn: &RustConnection, root: Window, win: Window, state_atom: Atom)
	-> Result<()>
{
	let atom = get_atom(conn, "_NET_WM_STATE_ABOVE")?;
	add_state(conn, root, win, state_atom, atom, 0, 0, 0)
}

#[inline]
fn remove_decoration(conn: &RustConnection, win: Window) -> Result<()>
{
	const PROP_MOTIF_WM_HINTS_ELEMENTS: u32 = 5;
	const MWM_HINTS_DECORATIONS: u32 = 1 << 1;

	let decoration_property = get_atom(conn, "_MOTIF_WM_HINTS")?;
	let mut data = vec![];
	push_u32(&mut data, MWM_HINTS_DECORATIONS);
	push_u32(&mut data, 0);
	push_u32(&mut data, 0);
	push_u32(&mut data, 0);
	push_u32(&mut data, 0);

	conn.change_property(
		PropMode::REPLACE,
		win,
		decoration_property,
		decoration_property,
		32,
		PROP_MOTIF_WM_HINTS_ELEMENTS,
		&data,
	)?.check()?;
	Ok(())
}

#[inline]
fn set_type(conn: &RustConnection, win: Window, win_type: &WindowType) -> Result<()>
{
	let win_type_prop = get_atom(conn, "_NET_WM_WINDOW_TYPE")?;
	let win_type_value = get_atom(conn, win_type.as_str())?;
	let mut data = vec![];
	push_u32(&mut data, win_type_value);
	conn.change_property(
		PropMode::REPLACE,
		win,
		win_type_prop,
		AtomEnum::ATOM,
		32,
		1,
		&data,
	)?.check()?;
	Ok(())
}

#[inline]
fn parse_geometry(geometry: &str) -> Result<WindowGeometry>
{
	let re = Regex::new(r"^((\d+)[xX](\d+))?(([+-])(\d+)([+-])(\d+))?$").unwrap();
	let captures = re.captures(geometry)
		.unwrap_or_else(|| panic!("Invalid geometry string: {geometry}"));
	let mut geometry = WindowGeometry {
		offset: None,
		size: None,
	};
	if let (Some(w), Some(h)) = (captures.get(2), captures.get(3)) {
		let w: u32 = w.as_str().parse()?;
		let h: u32 = h.as_str().parse()?;
		geometry.size = Some((w, h));
	}
	if let (Some(xs), Some(x), Some(ys), Some(y)) = (captures.get(5), captures.get(6), captures.get(7), captures.get(8)) {
		let x: i32 = x.as_str().parse()?;
		let xs = xs.as_str() == "-";
		let y: i32 = y.as_str().parse()?;
		let ys = ys.as_str() == "-";
		geometry.offset = Some((xs, x, ys, y));
	}
	Ok(geometry)
}

#[inline]
fn set_geometry(conn: &RustConnection, screen: &Screen, win: Window, geometry: &str) -> Result<()>
{
	let geometry = parse_geometry(geometry)?;
	let mut aux = ConfigureWindowAux::new();
	if let Some(size) = geometry.size {
		aux = aux.width(size.0).height(size.1);
	}
	if let Some(offset) = geometry.offset {
		let xs = offset.0;
		let mut x = offset.1;
		let ys = offset.2;
		let mut y = offset.3;
		let mut orig_win_size = None;
		if xs {
			let width = if let Some(size) = geometry.size {
				size.0 as i32
			} else {
				let size = conn.get_geometry(win)?
					.reply()?;
				let ow = size.width;
				let oh = size.height;
				orig_win_size = Some((ow, oh));
				ow as i32
			};
			x = screen.width_in_pixels as i32 - x - width;
		}
		if ys {
			let height = if let Some(size) = geometry.size {
				size.1 as i32
			} else if let Some((_, oh)) = orig_win_size {
				oh as i32
			} else {
				conn.get_geometry(win)?
					.reply()?.height as i32
			};
			y = screen.height_in_pixels as i32 - y - height;
		}
		aux = aux.x(x).y(y);
	}
	conn.configure_window(win, &aux)?.check()?;
	Ok(())
}

#[inline]
fn hide_taskbar_icon(conn: &RustConnection, root: Window, win: Window,
	state_atom: Atom) -> Result<()>
{
	let atom = get_atom(conn, "_NET_WM_STATE_SKIP_TASKBAR")?;
	add_state(conn, root, win, state_atom, atom, 0, 0, 0)
}

#[inline]
fn add_state(conn: &RustConnection, root: Window, win: Window, state_atom: Atom,
	v1: u32, v2: u32, v3: u32, v4: u32) -> Result<()>
{
	const _NET_WM_STATE_ADD: u32 = 1;
	send_message(conn, root, win, state_atom, [
		_NET_WM_STATE_ADD,
		v1, v2, v3, v4
	])?;
	Ok(())
}

#[inline]
fn get_atom(conn: &RustConnection, atom_name: &str) -> Result<Atom>
{
	Ok(conn.intern_atom(true, &Cow::Borrowed(atom_name.as_bytes()))?
		.reply()
		.unwrap_or_else(|_| panic!("Failed create atom: {atom_name}"))
		.atom)
}

#[cfg(test)]
mod test {
	use crate::parse_geometry;

	#[test]
	fn test_parse_geometry()
	{
		let g = parse_geometry("200x200+100-100").unwrap();
		assert_eq!(g.size.unwrap(), (200, 200));
		assert_eq!(g.offset.unwrap(), (false, 100, true, 100));
		let g = parse_geometry("200x200").unwrap();
		assert_eq!(g.size.unwrap(), (200, 200));
		assert!(g.offset.is_none());
		let g = parse_geometry("+100-100").unwrap();
		assert!(g.size.is_none());
		assert_eq!(g.offset.unwrap(), (false, 100, true, 100));
		let g = parse_geometry("-100-100").unwrap();
		assert!(g.size.is_none());
		assert_eq!(g.offset.unwrap(), (true, 100, true, 100));
	}
}
