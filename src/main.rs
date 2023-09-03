use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use anyhow::{anyhow, Result};
use clap::Parser;
use fork::Fork;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{Atom, ConnectionExt, PropMode, Window};
use x11rb::rust_connection::RustConnection;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Cli {
	#[clap(short, long, help = "icon file")]
	icon: PathBuf,
	#[clap(short, long, default_value = "10", help = "max seconds to wait for program to complete startup")]
	wait: u64,
	#[clap(short, long, help = "x11 program")]
	command: String,
	args: Vec<String>,
}

fn main() -> Result<()>
{
	let cli = Cli::parse();
	if !cli.icon.exists() {
		panic!("Icon file not exists: {:#?}", cli.icon)
	}

	match fork::daemon(false, true) {
		Ok(Fork::Parent(_)) => Ok(()),
		Ok(Fork::Child) => start(&cli.command, &cli.args, cli.wait, &cli.icon),
		Err(_) => Err(anyhow!("Failed fork")),
	}
	// start(&cli.command, &cli.args, cli.wait, &cli.icon)
}

#[inline]
fn start(command: &str, args: &Vec<String>, wait: u64, icon: &PathBuf) -> Result<()>
{
	let child = Command::new(command)
		.args(args)
		.spawn()?;
	let pid = child.id();
	let (conn, screen_num) = x11rb::connect(None)?;
	let screen = &conn.setup().roots[screen_num];
	let pid_property = conn.intern_atom(true, &Cow::Borrowed("_NET_WM_PID".as_bytes()))?
		.reply()
		.expect("Failed create pid property atom")
		.atom;
	let icon_property = conn.intern_atom(true, &Cow::Borrowed("_NET_WM_ICON".as_bytes()))?
		.reply()
		.expect("Failed create icon property atom")
		.atom;

	std::thread::sleep(Duration::from_millis(100));
	let mut i = 0;
	if let Some(win) = loop {
		if let Some(win) = window_with_pid(&conn, pid_property, icon_property, screen.root, pid as u32)? {
			break Some(win);
		}
		std::thread::sleep(Duration::from_millis(500));
		i += 1;
		if i >= wait * 2 {
			break None;
		}
	} {
		let (icon_data, icon_data_len) = load_icon(icon)?;
		for _ in i..wait * 2 {
			set_icon(&conn, win, icon_property, &icon_data, icon_data_len)?;
			std::thread::sleep(Duration::from_millis(500));
		}
	}
	Ok(())
}

fn window_with_pid(conn: &RustConnection, pid_property: Atom,
	icon_property: Atom, current: Window, pid: u32) -> Result<Option<Window>>
{
	let pid_result = conn.get_property(
		false,
		current,
		pid_property,
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
				icon_property,
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
		if let Some(win) = window_with_pid(conn, pid_property, icon_property, win, pid)? {
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

fn load_icon(icon: &PathBuf) -> Result<(Vec<u8>, u32)>
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
	Ok((data, length))
}

#[inline]
fn set_icon(conn: &RustConnection, win: Window, icon_property: Atom,
	icon_data: &[u8], icon_data_len: u32) -> Result<()>
{
	conn.change_property(
		PropMode::REPLACE,
		win,
		icon_property,
		Atom::from(6u8),
		32,
		icon_data_len,
		&icon_data,
	)?;
	Ok(())
}
