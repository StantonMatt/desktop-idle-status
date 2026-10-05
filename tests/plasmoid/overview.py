#!/usr/bin/python3
"""Build theme contact sheets from the isolated plasmoid captures."""
import math
import os
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

scratch = Path(os.environ.get("DIS_SCRATCH", Path.home() / ".cache/agent-scratch/desktop-idle-status"))
shots = Path(os.environ.get("DIS_SHOT_DIR", scratch / "plasmoid-shots"))
font = ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 18)
for theme in os.environ.get("DIS_THEMES", "light dark").split():
    captures = sorted(shots.glob(f"*-{theme}-full.png"))
    if not captures:
        continue
    width, height = 456, 480
    background, foreground = ("#2a2e32", "#eff0f1") if theme == "dark" else ("#eff0f1", "#232629")
    overview = Image.new("RGB", (width * 3, height * math.ceil(len(captures) / 3)), background)
    draw = ImageDraw.Draw(overview)
    for index, path in enumerate(captures):
        x, y = index % 3 * width, index // 3 * height
        label = path.name.removesuffix(f"-{theme}-full.png").replace("-", " ")
        draw.text((x + 12, y + 8), label, fill=foreground, font=font)
        with Image.open(path) as capture:
            overview.paste(capture, (x + 12, y + 36))
    overview.save(shots / f"{theme}-overview.png")
