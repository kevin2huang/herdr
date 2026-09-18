#!/usr/bin/env python3
import sys

CSI = "\x1b["
MONOKAI = CSI + "48;2;34;31;34m"
MOCHA = CSI + "48;2;30;30;46m"
GRAY = CSI + "38;2;91;89;92m"
GREEN = CSI + "38;2;169;220;118m"


def probe():
    output = [CSI + "0m" + MONOKAI + CSI + "2J" + CSI + "?25l"]

    def at(x, y, text):
        output.append(f"{CSI}{y};{x}H{text}")

    at(2, 2, GREEN + "Herdr border support probe")
    candidates = [
        ("Centered", "┌─┐││└─┘", MONOKAI),
        ("Outer edge", "🭽▔🭾▏▕🭼▁🭿", MOCHA),
        ("Inner edge", "🭿▁🭼▕▏🭾▔🭽", MONOKAI),
    ]
    for column, (name, glyphs, background) in enumerate(candidates):
        x, y, width, height = 2 + column * 24, 5, 20, 7
        at(x, y - 1, MONOKAI + GREEN + name)
        at(x, y, background + GRAY + glyphs[0] + glyphs[1] * (width - 2) + glyphs[2])
        for row in range(1, height - 1):
            at(
                x,
                y + row,
                background
                + GRAY
                + glyphs[3]
                + MOCHA
                + " " * (width - 2)
                + background
                + glyphs[4],
            )
        at(
            x,
            y + height - 1,
            background + GRAY + glyphs[5] + glyphs[6] * (width - 2) + glyphs[7],
        )
    at(2, 14, MONOKAI + GREEN + "Colored underline / overline")
    at(2, 16, MOCHA + CSI + "58;2;169;220;118m" + CSI + "4m" + " " * 20 + CSI + "24m")
    at(26, 16, MOCHA + GREEN + CSI + "53m" + " " * 20 + CSI + "55m" + MONOKAI)
    at(2, 19, GREEN + "Shared divider on Mocha")
    for y in range(21, 24):
        at(2, y, MOCHA + " " * 15 + GRAY + "│" + " " * 15 + MONOKAI)
    at(1, 25, CSI + "0m" + CSI + "?25h")
    return "".join(output)


if __name__ == "__main__":
    sys.stdout.write(probe())
