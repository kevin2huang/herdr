#!/usr/bin/env python3
import argparse
import json
from pathlib import Path


def color_runs(pixels, start=0):
    result = []
    for offset, color in enumerate(pixels, start):
        rgb = list(color)
        if result and result[-1]["rgb"] == rgb:
            result[-1]["end"] = offset + 1
            result[-1]["pixels"] += 1
        else:
            result.append({"start": offset, "end": offset + 1, "pixels": 1, "rgb": rgb})
    return result


def main():
    parser = argparse.ArgumentParser(
        description="Measure contiguous RGB pixel runs in a screenshot. End coordinates are exclusive."
    )
    parser.add_argument("image", type=Path)
    axis = parser.add_mutually_exclusive_group(required=True)
    axis.add_argument("--row", type=int)
    axis.add_argument("--column", type=int)
    parser.add_argument("--start", type=int, default=0)
    parser.add_argument("--end", type=int)
    args = parser.parse_args()
    try:
        from PIL import Image
    except ImportError:
        parser.error("Pillow is required for screenshot measurement")
    with Image.open(args.image) as source:
        image = source.convert("RGB")
        horizontal = args.row is not None
        fixed = args.row if horizontal else args.column
        length, breadth = image.size if horizontal else reversed(image.size)
        end = length if args.end is None else args.end
        if not (0 <= fixed < breadth and 0 <= args.start < end <= length):
            parser.error(f"scan is outside the {image.width} by {image.height} image")
        pixels = [
            image.getpixel((i, fixed) if horizontal else (fixed, i))
            for i in range(args.start, end)
        ]
        print(json.dumps(color_runs(pixels, args.start), indent=2))


if __name__ == "__main__":
    main()
