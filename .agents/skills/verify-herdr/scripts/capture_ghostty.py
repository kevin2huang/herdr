#!/usr/bin/env python3
import argparse
from dataclasses import asdict, dataclass
import json
import math
import os
from pathlib import Path
import shlex
import signal
import subprocess
import sys
import time
import uuid


LAUNCH_SCRIPT = r"""
on run argv
  set replayCommand to item 1 of argv
  tell application "Ghostty"
    set c to new surface configuration
    set command of c to replayCommand
    set wait after command of c to false
    set w to new window with configuration c
    set t to focused terminal of selected tab of w
    return (id of w) & linefeed & (id of t)
  end tell
end run
"""

WINDOW_SCRIPT = r"""
on run argv
  set wantedWindow to item 1 of argv
  tell application "Ghostty"
    set matches to every window whose id is wantedWindow
    if (count of matches) is 0 then return "absent"
    set terminalIds to {}
    repeat with t in terminals of item 1 of matches
      set end of terminalIds to id of t
    end repeat
    set AppleScript's text item delimiters to linefeed
    return "present" & linefeed & (terminalIds as text)
  end tell
end run
"""

INPUT_SCRIPT = r"""
on run argv
  set wantedTerminal to item 1 of argv
  tell application "Ghostty"
    set matches to every terminal whose id is wantedTerminal
    if (count of matches) is 0 then return "absent"
    input text "q" to item 1 of matches
    return "sent"
  end tell
end run
"""

CLOSE_SCRIPT = r"""
on run argv
  set wantedWindow to item 1 of argv
  set ownedTerminal to item 2 of argv
  tell application "Ghostty"
    set matches to every window whose id is wantedWindow
    if (count of matches) is 0 then return "already absent"
    set w to item 1 of matches
    repeat with t in terminals of w
      if id of t is not ownedTerminal then error "owned window contains an unowned terminal " & id of t
    end repeat
    close window w
    return "closed"
  end tell
end run
"""

CG_WINDOW_SCRIPT = r"""
import CoreGraphics
import Foundation
let title = ProcessInfo.processInfo.environment["VERIFY_HERDR_CAPTURE_TITLE"]!
let windows = CGWindowListCopyWindowInfo(.optionAll, kCGNullWindowID) as? [[String: Any]] ?? []
for window in windows where (window["kCGWindowOwnerName"] as? String) == "Ghostty" && (window["kCGWindowName"] as? String) == title {
    if let number = window["kCGWindowNumber"] { print(number) }
}
"""


@dataclass
class CaptureRecord:
    source: str
    image: str
    title: str
    ghostty_window_id: str = ""
    ghostty_terminal_id: str = ""
    core_graphics_window_id: str = ""
    capture_status: str = "pending"
    cleanup_status: str = "pending"
    error: str = ""


class Terminated(Exception):
    def __init__(self, signum):
        super().__init__(f"received signal {signum}")
        self.signum = signum


class GhosttyAutomation:
    def _osascript(self, script, *arguments):
        return subprocess.check_output(
            ["osascript", "-e", script, *arguments], text=True, timeout=10
        ).strip()

    def launch(self, command):
        lines = self._osascript(LAUNCH_SCRIPT, command).splitlines()
        if len(lines) != 2 or not all(lines):
            raise RuntimeError(f"Ghostty returned invalid ownership IDs: {lines!r}")
        return lines[0], lines[1]

    def window_terminals(self, window_id):
        lines = self._osascript(WINDOW_SCRIPT, window_id).splitlines()
        if not lines or lines[0] not in {"present", "absent"}:
            raise RuntimeError(f"Ghostty returned invalid window state: {lines!r}")
        return None if lines[0] == "absent" else lines[1:]

    def send_quit(self, terminal_id):
        return self._osascript(INPUT_SCRIPT, terminal_id)

    def close_window(self, window_id, terminal_id):
        return self._osascript(CLOSE_SCRIPT, window_id, terminal_id)

    def find_core_graphics_window(self, title):
        env = dict(os.environ, VERIFY_HERDR_CAPTURE_TITLE=title)
        lines = subprocess.check_output(
            ["swift", "-e", CG_WINDOW_SCRIPT], env=env, text=True, timeout=5
        ).splitlines()
        matches = [line for line in lines if line.isdigit()]
        if len(matches) > 1:
            raise RuntimeError(f"multiple Ghostty windows have capture title {title!r}")
        return matches[0] if matches else None


class CleanupError(RuntimeError):
    pass


def write_record(path, record):
    temporary = path.with_name(f".{path.name}.tmp")
    temporary.write_text(json.dumps(asdict(record), indent=2) + "\n")
    temporary.replace(path)


def wait_for_window(automation, title, timeout, interval=0.1):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        window_id = automation.find_core_graphics_window(title)
        if window_id:
            return window_id
        time.sleep(interval)
    raise TimeoutError(
        f"Ghostty preview did not become capturable within {timeout:g} seconds"
    )


def cleanup_owned_window(automation, window_id, terminal_id, timeout=5, interval=0.05):
    terminals = automation.window_terminals(window_id)
    if terminals is None:
        return "already absent"
    unowned = [candidate for candidate in terminals if candidate != terminal_id]
    if unowned:
        raise CleanupError(
            f"refusing to close owned window {window_id}; it contains unowned terminals {unowned}"
        )
    automation.send_quit(terminal_id)
    try:
        automation.close_window(window_id, terminal_id)
    except subprocess.CalledProcessError:
        if automation.window_terminals(window_id) is not None:
            raise
        return "closed and absent"
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if automation.window_terminals(window_id) is None:
            return "closed and absent"
        time.sleep(interval)
    raise CleanupError(f"owned Ghostty window {window_id} remains after close")


def capture(
    source, image, timeout=15, automation=None, screenshot=None, install_signals=True
):
    source = source.resolve()
    image = image.resolve()
    record_path = image.with_suffix(image.suffix + ".json")
    title = f"verify-herdr-{uuid.uuid4()}"
    record = CaptureRecord(str(source), str(image), title)
    automation = automation or GhosttyAutomation()
    replay = Path(__file__).with_name("replay_ansi.py")
    command = shlex.join([sys.executable, str(replay), str(source), "--title", title])
    screenshot = screenshot or (
        lambda window_id: subprocess.run(
            ["screencapture", "-x", "-o", "-l", window_id, str(image)],
            check=True,
            timeout=15,
        )
    )
    previous_handlers = {}
    launch_in_progress = False
    pending_signal = None

    def terminate(signum, _frame):
        nonlocal pending_signal
        if launch_in_progress:
            pending_signal = signum
            return
        raise Terminated(signum)

    if install_signals:
        for signum in (signal.SIGINT, signal.SIGTERM):
            previous_handlers[signum] = signal.signal(signum, terminate)

    failure = None
    try:
        try:
            launch_in_progress = True
            try:
                window_id, terminal_id = automation.launch(command)
                record.ghostty_window_id = window_id
                record.ghostty_terminal_id = terminal_id
                write_record(record_path, record)
            finally:
                launch_in_progress = False
            if pending_signal is not None:
                raise Terminated(pending_signal)
            record.core_graphics_window_id = wait_for_window(automation, title, timeout)
            write_record(record_path, record)
            screenshot(record.core_graphics_window_id)
            if not image.is_file() or image.stat().st_size == 0:
                raise RuntimeError(f"screencapture did not write {image}")
            record.capture_status = "captured"
        except BaseException as error:
            failure = error
            record.capture_status = "failed"
            record.error = str(error)
        finally:
            if record.ghostty_window_id:
                try:
                    record.cleanup_status = cleanup_owned_window(
                        automation,
                        record.ghostty_window_id,
                        record.ghostty_terminal_id,
                    )
                except BaseException as cleanup_error:
                    record.cleanup_status = "failed"
                    details = (
                        f"cleanup failed for Ghostty window {record.ghostty_window_id} "
                        f"and terminal {record.ghostty_terminal_id}: {cleanup_error}"
                    )
                    record.error = (
                        f"{record.error}; {details}" if record.error else details
                    )
                    if failure is None:
                        failure = CleanupError(details)
                    else:
                        failure.add_note(details)
            else:
                record.cleanup_status = "not launched"
            try:
                write_record(record_path, record)
            except BaseException as record_error:
                details = (
                    f"failed to write capture record {record_path}: {record_error}"
                )
                if failure is None:
                    failure = record_error
                failure.add_note(details)
    finally:
        for signum, handler in previous_handlers.items():
            signal.signal(signum, handler)
    if failure:
        raise failure
    return record_path


def main():
    parser = argparse.ArgumentParser(
        description="Capture isolated ANSI in an owned Ghostty window and close that window."
    )
    parser.add_argument("capture", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--timeout", type=float, default=15)
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("Ghostty native capture requires macOS")
    if not args.capture.is_file():
        parser.error(f"capture does not exist: {args.capture}")
    if not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("--timeout must be finite and greater than zero")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    try:
        record = capture(args.capture, args.output, args.timeout)
    except Terminated as error:
        print(f"error: {error}", file=sys.stderr)
        for note in getattr(error, "__notes__", []):
            print(f"error detail: {note}", file=sys.stderr)
        return 128 + error.signum
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"error: {error}", file=sys.stderr)
        for note in getattr(error, "__notes__", []):
            print(f"error detail: {note}", file=sys.stderr)
        return 1
    print(f"image: {args.output.resolve()}")
    print(f"record: {record}")
    print("owned Ghostty preview window: absent")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
