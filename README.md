# xicon(Start x11 program with custom icon)

## About

`xicon` is a tiny program to start a x11 program with custom icon and/or size.
With referenced at:
 https://specifications.freedesktop.org/wm-spec/wm-spec-1.3.html

## Build

cargo build --release

## Usage

xicon [--icon path-to-icon] [--size <max | min>] [--above] [--no-decoration] --command command [command args]

## License

GPLv2
