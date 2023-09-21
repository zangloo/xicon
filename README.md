# xicon(Start x11 program with custom icon)

## About

`xicon` is a tiny program to start a x11 program with custom icon and/or size.
With referenced at:
 https://specifications.freedesktop.org/wm-spec/wm-spec-1.3.html

## Build

cargo build --release

## Usage

```
xicon [OPTIONS] --command <COMMAND> [ARGS]...

Arguments:
  [ARGS]...

Options:
  -p, --property <PROPERTY>  window match property, <class|name>=<property value>
  -i, --icon <ICON>          icon file
  -s, --size <SIZE>          [possible values: max, min, fullscreen]
  -a, --above                always on top
  -d, --no-decoration        no decoration
  -t, --type <WIN_TYPE>      [possible values: desktop, dock, toolbar, menu, utility, splash, dialog, normal]
  -g, --geometry <GEOMETRY>  format: [<width>{xX}<height>][{+-}<xoffset>{+-}<yoffset>]
  -k, --no-taskbar-icon      hide window in taskbar
  -w, --wait <WAIT>          max seconds to wait for program to complete startup [default: 10]
  -c, --command <COMMAND>    x11 program to run
  -h, --help                 Print help
  -V, --version              Print version
```


## Examples

start xclock at right top without decoration and above all other windows
```
LC_ALL=zh_CN.UTF-8 xicon -d -a -g 150x30-250+0 -p name=xclock -c xclock -- -d -update 1 -strftime '%H:%M:%S %m/%d (%a)' -bg black -fg white
```

start rxvt max and no decoration
```
/usr/local/bin/xicon -d --size max -c /usr/bin/urxvt256c -- -name dt -T tile -e ssh -o requestTTY=yes ssh-host LANG=en_US.UTF-8 tmux a -t tmux-name
```

## License

GPLv2
