import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from capture_ghostty import CleanupError, Terminated, capture


class FakeAutomation:
    def __init__(self, terminals=None, close_error=None):
        self.window_id = "window-owned"
        self.terminal_id = "terminal-owned"
        self.terminals = [self.terminal_id] if terminals is None else terminals
        self.close_error = close_error
        self.actions = []

    def launch(self, command):
        self.actions.append(("launch", command))
        return self.window_id, self.terminal_id

    def find_core_graphics_window(self, title):
        self.actions.append(("find", title))
        return "417"

    def window_terminals(self, window_id):
        self.actions.append(("inspect", window_id))
        return None if self.terminals is None else list(self.terminals)

    def send_quit(self, terminal_id):
        self.actions.append(("quit", terminal_id))
        return "sent" if terminal_id in self.terminals else "absent"

    def close_window(self, window_id, terminal_id):
        self.actions.append(("close", window_id, terminal_id))
        if self.close_error:
            raise self.close_error
        self.terminals = None
        return "closed"


class SignalDuringLaunchAutomation(FakeAutomation):
    def launch(self, command):
        result = super().launch(command)
        os.kill(os.getpid(), signal.SIGTERM)
        return result


class DisappearingWindowAutomation(FakeAutomation):
    def close_window(self, window_id, terminal_id):
        self.actions.append(("close", window_id, terminal_id))
        self.terminals = None
        raise subprocess.CalledProcessError(1, ["osascript", "-e", "close"])


class CaptureGhosttyTests(unittest.TestCase):
    def run_capture(self, automation, screenshot, install_signals=False):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        source = root / "recording with spaces.ansi"
        image = root / "preview.png"
        source.write_bytes(b"ANSI fixture")
        self.capture_image = image
        record = capture(
            source,
            image,
            automation=automation,
            screenshot=lambda window_id: screenshot(image, window_id),
            install_signals=install_signals,
        )
        return image, record

    @staticmethod
    def save_image(image, _window_id):
        image.write_bytes(b"PNG fixture")

    def test_success_captures_owned_window_and_removes_it(self):
        automation = FakeAutomation()
        image, record_path = self.run_capture(automation, self.save_image)

        record = json.loads(record_path.read_text())
        self.assertEqual(image.read_bytes(), b"PNG fixture")
        self.assertEqual(record["core_graphics_window_id"], "417")
        self.assertEqual(record["capture_status"], "captured")
        self.assertEqual(record["cleanup_status"], "closed and absent")
        self.assertIsNone(automation.terminals)
        self.assertIn(("quit", "terminal-owned"), automation.actions)
        self.assertIn(("close", "window-owned", "terminal-owned"), automation.actions)
        launch_command = automation.actions[0][1]
        launch_arguments = shlex.split(launch_command)
        self.assertEqual(Path(launch_arguments[2]).name, "recording with spaces.ansi")
        self.assertEqual(launch_arguments[3], "--title")

    def test_capture_failure_still_removes_owned_window(self):
        automation = FakeAutomation()

        with self.assertRaisesRegex(RuntimeError, "capture failed"):
            self.run_capture(
                automation,
                lambda _image, _window_id: (_ for _ in ()).throw(
                    RuntimeError("capture failed")
                ),
            )

        self.assertIsNone(automation.terminals)

    def test_keyboard_interrupt_still_removes_owned_window(self):
        automation = FakeAutomation()

        with self.assertRaises(KeyboardInterrupt):
            self.run_capture(
                automation,
                lambda _image, _window_id: (_ for _ in ()).throw(KeyboardInterrupt()),
            )

        self.assertIsNone(automation.terminals)

    def test_sigterm_during_launch_records_ids_then_removes_owned_window(self):
        automation = SignalDuringLaunchAutomation()

        with self.assertRaises(Terminated) as raised:
            self.run_capture(automation, self.save_image, install_signals=True)

        self.assertEqual(raised.exception.signum, signal.SIGTERM)
        self.assertIsNone(automation.terminals)
        record = json.loads(self.capture_image.with_suffix(".png.json").read_text())
        self.assertEqual(record["ghostty_window_id"], "window-owned")
        self.assertEqual(record["cleanup_status"], "closed and absent")

    def test_already_exited_terminal_tombstone_window_is_closed(self):
        automation = FakeAutomation(terminals=[])
        _, record_path = self.run_capture(automation, self.save_image)

        record = json.loads(record_path.read_text())
        self.assertEqual(record["ghostty_terminal_id"], "terminal-owned")
        self.assertEqual(record["cleanup_status"], "closed and absent")
        self.assertIn(("quit", "terminal-owned"), automation.actions)
        self.assertIsNone(automation.terminals)

    def test_window_disappearing_during_close_counts_as_closed(self):
        automation = DisappearingWindowAutomation()
        _, record_path = self.run_capture(automation, self.save_image)

        record = json.loads(record_path.read_text())
        self.assertEqual(record["cleanup_status"], "closed and absent")
        self.assertIsNone(automation.terminals)
        self.assertEqual(
            automation.actions[-2:],
            [("close", "window-owned", "terminal-owned"), ("inspect", "window-owned")],
        )

    def test_unrelated_terminal_blocks_window_close(self):
        automation = FakeAutomation(terminals=["terminal-owned", "terminal-user"])

        with self.assertRaisesRegex(CleanupError, "unowned terminals"):
            self.run_capture(automation, self.save_image)

        self.assertEqual(automation.terminals, ["terminal-owned", "terminal-user"])
        self.assertNotIn(("quit", "terminal-owned"), automation.actions)
        self.assertNotIn(
            ("close", "window-owned", "terminal-owned"), automation.actions
        )

    def test_cleanup_failure_reports_owned_ids_for_recovery(self):
        automation = FakeAutomation(close_error=RuntimeError("automation denied close"))

        with self.assertRaisesRegex(
            CleanupError,
            "window window-owned and terminal terminal-owned: automation denied close",
        ):
            self.run_capture(automation, self.save_image)

        self.assertEqual(automation.terminals, ["terminal-owned"])
        record = json.loads(self.capture_image.with_suffix(".png.json").read_text())
        self.assertEqual(record["ghostty_window_id"], "window-owned")
        self.assertEqual(record["ghostty_terminal_id"], "terminal-owned")
        self.assertEqual(record["cleanup_status"], "failed")

    def test_success_with_final_record_write_failure_fails_visibly(self):
        with patch(
            "capture_ghostty.write_record",
            side_effect=[None, None, RuntimeError("record disk full")],
        ):
            with self.assertRaisesRegex(RuntimeError, "record disk full") as raised:
                self.run_capture(FakeAutomation(), self.save_image)

        self.assertTrue(
            any(
                "failed to write capture record" in note
                for note in getattr(raised.exception, "__notes__", [])
            )
        )

    def test_record_write_failure_restores_handlers_and_preserves_original_error(self):
        original_error = RuntimeError("capture failed first")
        automation = FakeAutomation(close_error=RuntimeError("automation denied close"))
        previous = {
            signal.SIGINT: signal.getsignal(signal.SIGINT),
            signal.SIGTERM: signal.getsignal(signal.SIGTERM),
        }

        def sigint_handler(_signum, _frame):
            return None

        def sigterm_handler(_signum, _frame):
            return None

        expected = {signal.SIGINT: sigint_handler, signal.SIGTERM: sigterm_handler}
        try:
            for signum, handler in expected.items():
                signal.signal(signum, handler)
            with patch(
                "capture_ghostty.write_record",
                side_effect=[None, None, RuntimeError("record disk full")],
            ):
                with self.assertRaises(RuntimeError) as raised:
                    self.run_capture(
                        automation,
                        lambda _image, _window_id: (_ for _ in ()).throw(
                            original_error
                        ),
                        install_signals=True,
                    )

            self.assertIs(raised.exception, original_error)
            self.assertEqual(
                {signum: signal.getsignal(signum) for signum in expected}, expected
            )
            notes = getattr(raised.exception, "__notes__", [])
            self.assertTrue(
                any(
                    "window window-owned and terminal terminal-owned" in note
                    for note in notes
                )
            )
            self.assertTrue(any("record disk full" in note for note in notes))
        finally:
            for signum, handler in previous.items():
                signal.signal(signum, handler)


if __name__ == "__main__":
    unittest.main()
