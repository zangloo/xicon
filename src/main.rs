use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use anyhow::{anyhow, Result};
use clap::Parser;
use fork::Fork;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{Atom, ClientMessageData, ClientMessageEvent, ConnectionExt, EventMask, PropMode, Window};
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

	match fork::daemon(false, true) {
		Ok(Fork::Parent(_)) => Ok(()),
		Ok(Fork::Child) => start(
			&cli.command,
			&cli.args,
			cli.wait,
			&cli.icon,
			&cli.size),
		Err(_) => Err(anyhow!("Failed fork")),
	}
	// start(&cli.command, &cli.args, cli.wait, &cli.icon)
}

struct IconData {
	data: Vec<u8>,
	length: u32,
}

struct PropertieAtoms {
	pid: Atom,
	set_icon: Atom,
	state: Atom,
	vertical: Atom,
	horizontal: Atom,
	change_state: Atom,
	iconic: Atom,
}

#[inline]
fn start(command: &str, args: &Vec<String>, wait: u64,
	icon_path: &Option<PathBuf>, size: &Option<WindowSize>) -> Result<()>
{
	let child = Command::new(command)
		.args(args)
		.spawn()?;
	let pid = child.id();
	let (conn, screen_num) = x11rb::connect(None)?;
	let screen = &conn.setup().roots[screen_num];
	let properties = PropertieAtoms {
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
	};

	std::thread::sleep(Duration::from_millis(100));
	let mut i = 0;
	if let Some(win) = loop {
		if let Some(win) = window_with_pid(&conn, &properties, screen.root, pid)? {
			break Some(win);
		}
		std::thread::sleep(Duration::from_millis(500));
		i += 1;
		if i >= wait * 2 {
			break None;
		}
	} {
		let icon = if let Some(icon_path) = icon_path {
			Some(load_icon(icon_path)?)
		} else {
			None
		};
		for _ in i..wait * 2 {
			if let Some(icon) = &icon {
				set_icon(&conn, win, &properties, &icon)?;
			}
			if let Some(size) = &size {
				set_size(&conn, screen.root, win, &size, &properties)?;
			}
			std::thread::sleep(Duration::from_millis(500));
		}
	}
	Ok(())
}

fn window_with_pid(conn: &RustConnection, properties: &PropertieAtoms,
	current: Window, pid: u32) -> Result<Option<Window>>
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
		let win_pid = pid_reply.value32()
			.expect("Invalid replay")
			.next()
			.expect("No pid exists in result");
		if win_pid == pid {
			let icon_reply = conn.get_property(false,
				current,
				properties.set_icon,
				Atom::from(6u8),
				0, 1,
			)?.reply()?;
			// the window with icon
			if icon_reply.length == 1 {
				if icon_reply.value32()
					.expect("Invalid icon replay")
					.next()
					.is_some() {
					return Ok(Some(current));
				}
			}
		}
	}
	let tree_result = conn.query_tree(current)?;
	for win in tree_result.reply()?.children {
		if let Some(win) = window_with_pid(conn, &properties, win, pid)? {
			return Ok(Some(win));
		}
	}
	Ok(None)
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
fn set_icon(conn: &RustConnection, win: Window, properties: &PropertieAtoms,
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

fn set_size(conn: &RustConnection, root: Window, win: Window,
	size: &WindowSize, properties: &PropertieAtoms)
	-> Result<()>
{
	match size {
		WindowSize::Max => {
			let data = ClientMessageData::from(
				[
					1,              // _NET_WM_STATE_ADD
					properties.vertical,
					properties.horizontal,
					1,              // application ??
					0,
				]
			);
			let event = ClientMessageEvent::new(
				32, win, properties.state, data);

			conn.send_event(
				true,
				root,
				EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
				event,
			)?.check()?;
		}
		WindowSize::Min => {
			let data = ClientMessageData::from(
				[
					properties.iconic,
					0, 0, 0, 0,
				]
			);
			let event = ClientMessageEvent::new(
				32, win, properties.change_state, data);

			conn.send_event(
				true,
				root,
				EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
				event,
			)?.check()?;
		}
	}
	Ok(())
}

