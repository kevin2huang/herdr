import json
import os
from pathlib import Path
import pty
import select
import subprocess
import sys
import tempfile
import termios
import time
import unittest

from pixel_runs import color_runs

SCRIPTS = Path(__file__).parent


class VisualToolsTests(unittest.TestCase):
    def test_pixel_runs_keep_coordinates_and_single_pixel_strokes(self):
        self.assertEqual(
            color_runs([(34, 31, 34)] * 2 + [(91, 89, 92)] + [(30, 30, 46)] * 3, 10),
            [
                {"start": 10, "end": 12, "pixels": 2, "rgb": [34, 31, 34]},
                {"start": 12, "end": 13, "pixels": 1, "rgb": [91, 89, 92]},
                {"start": 13, "end": 16, "pixels": 3, "rgb": [30, 30, 46]},
            ],
        )

    def test_pixel_cli_measures_both_axes(self):
        from PIL import Image

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "fixture.png"
            image = Image.new("RGB", (4, 3), (34, 31, 34))
            image.putpixel((2, 1), (30, 30, 46))
            image.save(path)
            for axis, index, lengths in [
                ("--row", "1", [2, 1, 1]),
                ("--column", "2", [1, 1, 1]),
            ]:
                result = json.loads(
                    subprocess.check_output(
                        [
                            sys.executable,
                            str(SCRIPTS / "pixel_runs.py"),
                            str(path),
                            axis,
                            index,
                        ]
                    )
                )
                self.assertEqual([run["pixels"] for run in result], lengths)
                self.assertEqual(result[1]["rgb"], [30, 30, 46])
            invalid = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPTS / "pixel_runs.py"),
                    str(path),
                    "--row",
                    "3",
                ],
                capture_output=True,
            )
            self.assertEqual(invalid.returncode, 2)
            self.assertIn(b"outside the 4 by 3 image", invalid.stderr)

    def test_replay_does_not_echo_terminal_replies_and_restores_tty(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Path(directory) / "raw.ansi"
            capture.write_bytes(b"\x1b]11;?\x1b\\REPLAY_READY")
            master, slave = pty.openpty()
            process = subprocess.Popen(
                [sys.executable, str(SCRIPTS / "replay_ansi.py"), str(capture)],
                stdin=slave,
                stdout=slave,
                stderr=slave,
            )
            try:
                output = b""
                deadline = time.monotonic() + 5
                while b"REPLAY_READY" not in output and time.monotonic() < deadline:
                    if select.select([master], [], [], 0.1)[0]:
                        output += os.read(master, 8192)
                self.assertIn(b"REPLAY_READY", output)
                os.write(master, b"\x1b]11;rgb:2222/1f1f/2222\x1b\\q")
                self.assertEqual(process.wait(timeout=5), 0)
                while select.select([master], [], [], 0.1)[0]:
                    output += os.read(master, 8192)
                self.assertNotIn(b"rgb:2222", output)
                flags = termios.tcgetattr(slave)[3]
                for flag in [termios.ECHO, termios.ICANON, termios.ISIG]:
                    self.assertTrue(flags & flag)
                os.write(master, b"cooked\n")
                self.assertTrue(select.select([slave], [], [], 1)[0])
                self.assertEqual(os.read(slave, 1024), b"cooked\n")
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)
                os.close(master)
                os.close(slave)


if __name__ == "__main__":
    unittest.main()
