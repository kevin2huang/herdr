#!/usr/bin/env python3
import argparse
import os
from pathlib import Path
import sys
import termios
import tty


def main():
    parser = argparse.ArgumentParser(
        description="Replay captured ANSI in a disposable terminal. Press q to exit."
    )
    parser.add_argument("capture", type=Path)
    parser.add_argument("--title")
    args = parser.parse_args()
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        parser.error("run in a disposable interactive terminal, not a pipe")
    capture = args.capture.read_bytes()
    fd = sys.stdin.fileno()
    saved = termios.tcgetattr(fd)
    try:
        # Raw mode prevents echoed OSC/CSI query replies from corrupting the capture.
        tty.setraw(fd)
        sys.stdout.buffer.write(b"\x1b[?1049h\x1b[0m\x1b[48;2;34;31;34m\x1b[2J\x1b[H")
        sys.stdout.buffer.write(capture + b"\x1b[?25l")
        if args.title:
            sys.stdout.buffer.write(f"\x1b]0;{args.title}\x07".encode())
        sys.stdout.flush()
        while True:
            data = os.read(fd, 1024)
            if not data or b"q" in data or b"\x03" in data:
                break
    finally:
        sys.stdout.write("\x1b[0m\x1b[?25h\x1b[?1049l")
        sys.stdout.flush()
        termios.tcsetattr(fd, termios.TCSANOW, saved)


if __name__ == "__main__":
    main()
