use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::SystemTime;
use anyhow::Result;
use clap::Parser;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{Atom, ChangeWindowAttributesAux, ClientMessageEvent, ConnectionExt, EventMask, PropMode, Window};
use x11rb::rust_connection::RustConnection;

#[derive(clap::ValueEnum, Clone, Debug)]
enum WindowSize {
	Max,
	Min,
}

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Cli {
	#[clap(short, long, help = "icon file")]
	icon: Option<PathBuf>,
	#[clap(short, long, help = "window size max/min", value_enum)]
	size: Option<WindowSize>,
	#[clap(short, long, help = "window always on top")]
	above: bool,
	#[clap(short, long, default_value = "10", help = "max seconds to wait for program to complete startup")]
	wait: u64,
	#[clap(short, long, help = "x11 program")]
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

	start(&cli.command, &cli.args, cli.wait, &cli.icon, &cli.size, cli.above)
}

struct IconData {
	data: Vec<u8>,
	length: u32,
}

struct PropertyAtoms {
	pid: Atom,
	set_icon: Atom,
	state: Atom,
	vertical: Atom,
	horizontal: Atom,
	change_state: Atom,
	iconic: Atom,
	above: Atom,
}

#[inline]
fn start(command: &str, args: &Vec<String>, wait: u64,
	icon_path: &Option<PathBuf>, size: &Option<WindowSize>, above: bool)
	-> Result<()>
{
	let (conn, screen_num) = x11rb::connect(None)?;
	let screen = &conn.setup().roots[screen_num];
	let properties = PropertyAtoms {
		pid: conn.intern_atom(true, &Cow::Borrowed("_NET_WM_PID".as_bytes()))?
			.reply()
			.expect("Failed create pid property atom")
			.atom,
		set_icon: conn.intern_atom(true, &Cow::Borrowed("_NET_WM_ICON".as_bytes()))?
			.reply()
			.expect("Failed create icon property atom")
			.atom,
		state: conn.intern_atom(true, &Cow::Borrowed("_NET_WM_STATE".as_bytes()))?
			.reply()
			.expect("Failed create state property atom")
			.atom,
		vertical: conn.intern_atom(true, &Cow::Borrowed("_NET_WM_STATE_MAXIMIZED_VERT".as_bytes()))?
			.reply()
			.expect("Failed create vert property atom")
			.atom,
		horizontal: conn.intern_atom(true, &Cow::Borrowed("_NET_WM_STATE_MAXIMIZED_HORZ".as_bytes()))?
			.reply()
			.expect("Failed create hor property atom")
			.atom,
		change_state: conn.intern_atom(true, &Cow::Borrowed("WM_CHANGE_STATE".as_bytes()))?
			.reply()
			.expect("Failed create min property atom")
			.atom,
		iconic: Atom::from(3u8),    // IconicState
		above: conn.intern_atom(true, &Cow::Borrowed("_NET_WM_STATE_ABOVE".as_bytes()))?
			.reply()
			.expect("Failed create min property atom")
			.atom,
	};

	let mut aux = ChangeWindowAttributesAux::new();
	aux.event_mask = Some(EventMask::SUBSTRUCTURE_NOTIFY);
	conn.change_window_attributes(screen.root, &aux)?.check()?;
	conn.flush()?;
	let child = Command::new(command)
		.args(args)
		.spawn()?;
	let pid = child.id();
	let start = SystemTime::now();
	loop {
		let event = conn.wait_for_event()?;
		match event {
			Event::ReparentNotify(event) => {
				let win = event.window;
				if let Some(win_pid) = get_pid(&conn, event.window, &properties)? {
					if win_pid == pid {
						if let Some(icon) = icon_path {
							let icon = load_icon(icon)?;
							set_icon(&conn, win, &properties, &icon)?;
						}
						if let Some(size) = &size {
							set_size(&conn, screen.root, win, &size, &properties)?;
						}
						if above {
							set_above(&conn, screen.root, win, &properties)?;
						}
						break;
					}
				}
			}
			_ => {}
		}
		let now = SystemTime::now();
		let duration = now.duration_since(start)
			.expect("Clock may have gone backwards");
		if duration.as_secs() > wait {
			eprintln!("Failed to detect command windows in {wait} seconds, quit.");
			break;
		}
	}
	Ok(())
}

fn get_pid(conn: &RustConnection, current: Window, properties: &PropertyAtoms)
	-> Result<Option<u32>>
{
	let pid_result = conn.get_property(
		false,
		current,
		properties.pid,
		Atom::from(6u8),
		0, 1,
	)?;
	let pid_reply = pid_result.reply()?;
	if pid_reply.length == 1 {
		let pid = pid_reply.value32()
			.expect("Invalid replay")
			.next()
			.expect("No pid exists in result");
		Ok(Some(pid))
	} else {
		Ok(None)
	}
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
	loop {
		match slice {
			[r, g, b, a, rest @ ..] => {
				data.push(*b);
				data.push(*g);
				data.push(*r);
				data.push(*a);
				slice = rest;
			}
			_ => break,
		}
	}
	let length = width * height + 2;
	Ok(IconData { data, length })
}

#[inline]
fn set_icon(conn: &RustConnection, win: Window, properties: &PropertyAtoms,
	icon: &IconData) -> Result<()>
{
	conn.change_property(
		PropMode::REPLACE,
		win,
		properties.set_icon,
		Atom::from(6u8),
		32,
		icon.length,
		&icon.data,
	)?;
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

fn set_size(conn: &RustConnection, root: Window, win: Window,
	size: &WindowSize, properties: &PropertyAtoms)
	-> Result<()>
{
	match size {
		WindowSize::Max => {
			send_message(conn, root, win, properties.state, [
				1,              // _NET_WM_STATE_ADD
				properties.vertical,
				properties.horizontal,
				1,              // application ??
				0,
			])?;
		}
		WindowSize::Min => {
			send_message(conn, root, win, properties.change_state, [
				properties.iconic,
				0, 0, 0, 0,
			])?;
		}
	}
	Ok(())
}

fn set_above(conn: &RustConnection, root: Window, win: Window, properties: &PropertyAtoms)
	-> Result<()>
{
	send_message(conn, root, win, properties.state, [
		1,
		properties.above,
		0, 0, 0,
	])?;
	Ok(())
}
