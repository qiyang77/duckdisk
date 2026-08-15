#!/usr/bin/env python3
"""Render website media from one verified DuckDisk capture set."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

import cv2
import numpy as np
from PIL import Image, ImageDraw, ImageFont


CANVAS_WIDTH = 960
CANVAS_HEIGHT = 600
APP_WIDTH = 868
APP_HEIGHT = 550
APP_X = (CANVAS_WIDTH - APP_WIDTH) // 2
APP_Y = 8
FPS = 10
SF_FONT = "/System/Library/Fonts/SFNS.ttf"

BASE_FILES = {
    "home": "00-home.jpeg",
    "cloud_result": "01-cloud-result.jpeg",
    "overview": "02-overview.jpeg",
    "review": "05-review-clean.jpeg",
    "queued": "06-queued-clean.jpeg",
}

CURSOR_REPAIRS = {
    "home": ((482, 20), (360, 20)),
    "cloud_result": ((799, 489), (900, 489)),
    "overview": ((985, 575), (835, 575)),
    # These fresh captures park the real pointer in the empty center pane, so
    # removing it cannot touch filenames or any other UI text.
    "review": ((700, 400), (600, 400)),
    "queued": ((700, 400), (600, 400)),
    "scan": ((1014, 67), (915, 67)),
    "result": ((1014, 67), (855, 67)),
}


def smoothstep(value: float) -> float:
    value = max(0.0, min(1.0, value))
    return value * value * (3.0 - 2.0 * value)


def cover(image: np.ndarray, width: int, height: int) -> np.ndarray:
    source_height, source_width = image.shape[:2]
    scale = max(width / source_width, height / source_height)
    resized = cv2.resize(
        image,
        (round(source_width * scale), round(source_height * scale)),
        interpolation=cv2.INTER_LANCZOS4,
    )
    y = (resized.shape[0] - height) // 2
    x = (resized.shape[1] - width) // 2
    return resized[y : y + height, x : x + width]


def rounded_mask(width: int, height: int, radius: int) -> np.ndarray:
    mask = np.zeros((height, width), dtype=np.uint8)
    cv2.rectangle(mask, (radius, 0), (width - radius, height), 255, -1)
    cv2.rectangle(mask, (0, radius), (width, height - radius), 255, -1)
    for x, y in (
        (radius, radius),
        (width - radius - 1, radius),
        (radius, height - radius - 1),
        (width - radius - 1, height - radius - 1),
    ):
        cv2.circle(mask, (x, y), radius, 255, -1, cv2.LINE_AA)
    return mask


def restore_window_controls(image: np.ndarray) -> np.ndarray:
    """Replace the capture badge with the native macOS traffic lights."""
    result = image.copy()
    for y in range(0, 38):
        color = np.median(result[y, 88:190], axis=0).astype(np.uint8)
        result[y, 0:86] = color

    controls = (
        ((19, 19), (87, 95, 255)),
        ((39, 19), (46, 188, 254)),
        ((59, 19), (64, 200, 40)),
    )
    for center, color in controls:
        cv2.circle(result, center, 7, (20, 22, 24), -1, cv2.LINE_AA)
        cv2.circle(result, center, 6, color, -1, cv2.LINE_AA)
    return result


def repair_cursor(
    image: np.ndarray,
    center: tuple[int, int],
    source_center: tuple[int, int],
    radius: int = 46,
) -> np.ndarray:
    result = image.copy()
    x, y = center
    source_x, source_y = source_center
    x0, x1 = max(0, x - radius), min(image.shape[1], x + radius)
    y0, y1 = max(0, y - radius), min(image.shape[0], y + radius)
    sx0 = source_x - (x - x0)
    sy0 = source_y - (y - y0)
    sx1 = sx0 + (x1 - x0)
    sy1 = sy0 + (y1 - y0)
    patch = image[sy0:sy1, sx0:sx1].copy()
    target = result[y0:y1, x0:x1]
    if patch.shape != target.shape:
        return result

    mask = np.zeros(target.shape[:2], dtype=np.uint8)
    cv2.circle(mask, (x - x0, y - y0), radius - 8, 255, -1, cv2.LINE_AA)
    mask = cv2.GaussianBlur(mask, (0, 0), 7)
    alpha = (mask.astype(np.float32) / 255.0)[..., None]
    result[y0:y1, x0:x1] = (
        patch.astype(np.float32) * alpha
        + target.astype(np.float32) * (1.0 - alpha)
    ).astype(np.uint8)
    return result


def clean_capture(image: np.ndarray, repair_key: str) -> np.ndarray:
    result = restore_window_controls(image)
    result = repair_cursor(result, *CURSOR_REPAIRS[repair_key])
    # Keep the complete 1200 x 760 capture. Rounded transparency is applied
    # only when writing static PNGs, so no UI content is cropped or enlarged.
    return result


def compose_static(wallpaper: np.ndarray, source: np.ndarray) -> np.ndarray:
    frame = cover(wallpaper, 1200, 760)
    app_width, app_height = 1080, 684
    app_x, app_y = 60, 38
    window = cv2.resize(
        source, (app_width, app_height), interpolation=cv2.INTER_LANCZOS4
    )
    mask = rounded_mask(app_width, app_height, 14)
    alpha = (mask.astype(np.float32) / 255.0)[..., None]
    target = frame[app_y : app_y + app_height, app_x : app_x + app_width]
    frame[app_y : app_y + app_height, app_x : app_x + app_width] = (
        window.astype(np.float32) * alpha
        + target.astype(np.float32) * (1.0 - alpha)
    ).astype(np.uint8)
    return frame


def compose_window(wallpaper: np.ndarray, source: np.ndarray) -> np.ndarray:
    """Place the app on wallpaper without the previous artificial black shadow."""
    frame = wallpaper.copy()
    window = cv2.resize(
        source, (APP_WIDTH, APP_HEIGHT), interpolation=cv2.INTER_LANCZOS4
    )
    mask = rounded_mask(APP_WIDTH, APP_HEIGHT, 11)
    alpha = (mask.astype(np.float32) / 255.0)[..., None]
    target = frame[APP_Y : APP_Y + APP_HEIGHT, APP_X : APP_X + APP_WIDTH]
    frame[APP_Y : APP_Y + APP_HEIGHT, APP_X : APP_X + APP_WIDTH] = (
        window.astype(np.float32) * alpha
        + target.astype(np.float32) * (1.0 - alpha)
    ).astype(np.uint8)
    return frame


def source_to_canvas(point: tuple[float, float]) -> tuple[float, float]:
    return (
        APP_X + point[0] / 1200.0 * APP_WIDTH,
        APP_Y + point[1] / 760.0 * APP_HEIGHT,
    )


def interpolate_point(
    start: tuple[float, float], end: tuple[float, float], amount: float
) -> tuple[float, float]:
    amount = smoothstep(amount)
    return (
        start[0] + (end[0] - start[0]) * amount,
        start[1] + (end[1] - start[1]) * amount,
    )


def draw_cursor(
    frame: np.ndarray, point: tuple[float, float], cursor: np.ndarray
) -> None:
    """Draw the native NSCursor.arrow image at its top-left hotspot."""
    overlay_rgba(frame, cursor, round(point[0]), round(point[1]))


def draw_click_pulse(frame: np.ndarray, point: tuple[float, float], amount: float) -> None:
    radius = round(8 + 18 * amount)
    alpha = 1.0 - amount
    overlay = frame.copy()
    cv2.circle(
        overlay,
        (round(point[0]), round(point[1])),
        radius,
        (255, 255, 255),
        1,
        cv2.LINE_8,
    )
    cv2.addWeighted(overlay, alpha, frame, 1.0 - alpha, 0.0, frame)


def drag_chip() -> np.ndarray:
    font = ImageFont.truetype(SF_FONT, 15)
    text = "Old Screen Recording.mov"
    probe = Image.new("RGBA", (1, 1))
    draw = ImageDraw.Draw(probe)
    box = draw.textbbox((0, 0), text, font=font)
    width = box[2] - box[0] + 24
    height = box[3] - box[1] + 16
    image = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    draw.rounded_rectangle(
        (0, 0, width - 1, height - 1),
        radius=7,
        fill=(71, 22, 28, 238),
        outline=(245, 116, 126, 230),
        width=1,
    )
    draw.text((12, 8 - box[1]), text, font=font, fill=(255, 241, 242, 255))
    return cv2.cvtColor(np.array(image), cv2.COLOR_RGBA2BGRA)


def overlay_rgba(frame: np.ndarray, image: np.ndarray, x: int, y: int) -> None:
    height, width = image.shape[:2]
    x0, y0 = max(0, x), max(0, y)
    x1, y1 = min(frame.shape[1], x + width), min(frame.shape[0], y + height)
    source = image[y0 - y : y1 - y, x0 - x : x1 - x]
    alpha = source[..., 3:4].astype(np.float32) / 255.0
    target = frame[y0:y1, x0:x1].astype(np.float32)
    frame[y0:y1, x0:x1] = (
        source[..., :3].astype(np.float32) * alpha + target * (1.0 - alpha)
    ).astype(np.uint8)


def blend(first: np.ndarray, second: np.ndarray, amount: float) -> np.ndarray:
    amount = smoothstep(amount)
    return cv2.addWeighted(first, 1.0 - amount, second, amount, 0.0)


def scan_frames(
    home: np.ndarray,
    scan_states: list[np.ndarray],
    wallpaper: np.ndarray,
    cursor_image: np.ndarray,
) -> list[np.ndarray]:
    frames: list[np.ndarray] = []
    cursor_start = source_to_canvas((500.0, 28.0))
    cursor_end = source_to_canvas((610.0, 132.0))

    # Show the complete action: approach the Macintosh HD row and click it.
    for index in range(20):
        frame = compose_window(wallpaper, home)
        cursor = interpolate_point(cursor_start, cursor_end, index / 15.0)
        draw_cursor(frame, cursor, cursor_image)
        if index >= 15:
            draw_click_pulse(frame, cursor_end, (index - 15) / 5.0)
        frames.append(frame)

    # Each entry is a distinct real Computer Use capture. Keeping one frame
    # per state gives a continuous progress animation instead of six jumps.
    frames.extend(compose_window(wallpaper, source) for source in scan_states)
    frames.extend(
        compose_window(wallpaper, scan_states[-1]) for _ in range(20)
    )
    return frames


def delete_frames(
    review: np.ndarray,
    queued: np.ndarray,
    wallpaper: np.ndarray,
    cursor_image: np.ndarray,
) -> list[np.ndarray]:
    frames: list[np.ndarray] = []
    chip = drag_chip()
    start = source_to_canvas((135.0, 205.0))
    end = source_to_canvas((1018.0, 642.0))
    target_left = APP_X + round(848 / 1200.0 * APP_WIDTH)
    target_top = APP_Y + round(620 / 760.0 * APP_HEIGHT)
    target_right = APP_X + round(1192 / 1200.0 * APP_WIDTH)
    target_bottom = APP_Y + round(663 / 760.0 * APP_HEIGHT)

    for _ in range(12):
        frame = compose_window(wallpaper, review)
        draw_cursor(frame, start, cursor_image)
        frames.append(frame)
    # Animate the dragged item with the same restrained cursor used by GIF 1.
    # No modal dimming or artificial window shadow is introduced.
    for index in range(26):
        frame = compose_window(wallpaper, review)
        amount = index / 25.0
        point = interpolate_point(start, end, amount)
        cv2.rectangle(
            frame,
            (target_left, target_top),
            (target_right, target_bottom),
            (82, 103, 232),
            2,
            cv2.LINE_AA,
        )
        draw_cursor(frame, point, cursor_image)
        overlay_rgba(frame, chip, round(point[0] + 14), round(point[1] + 14))
        frames.append(frame)

    for index in range(5):
        source = blend(review, queued, index / 4.0)
        frame = compose_window(wallpaper, source)
        draw_cursor(frame, end, cursor_image)
        frames.append(frame)
    for _ in range(28):
        frame = compose_window(wallpaper, queued)
        draw_cursor(frame, end, cursor_image)
        frames.append(frame)
    return frames


def save_gif(frames: list[np.ndarray], path: Path, disposal: int) -> None:
    rgb_frames = [
        Image.fromarray(cv2.cvtColor(frame, cv2.COLOR_BGR2RGB)) for frame in frames
    ]
    # A separate 256-color palette per frame keeps the app UI and pointer
    # accurate. Disabling color dithering avoids colored noise around text,
    # controls, and the cursor.
    quantized = [
        frame.quantize(
            colors=256,
            method=Image.Quantize.MEDIANCUT,
            dither=Image.Dither.NONE,
        )
        for frame in rgb_frames
    ]
    quantized[0].save(
        path,
        save_all=True,
        append_images=quantized[1:],
        duration=round(1000 / FPS),
        loop=0,
        optimize=False,
        disposal=disposal,
    )


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def read_capture(path: Path, repair_key: str) -> np.ndarray:
    image = cv2.imread(str(path), cv2.IMREAD_COLOR)
    if image is None:
        raise FileNotFoundError(path)
    if image.shape[:2] != (760, 1200):
        raise ValueError(f"unexpected capture dimensions for {path}: {image.shape}")
    return clean_capture(image, repair_key)


def render(asset_root: Path, output_dir: Path, wallpaper_path: Path) -> None:
    raw_dir = asset_root / "raw"
    images = {
        key: read_capture(raw_dir / filename, key)
        for key, filename in BASE_FILES.items()
    }

    scan_paths = sorted(raw_dir.glob("scan-*.jpeg"))
    if len(scan_paths) < 40:
        raise ValueError(f"expected dense scan sequence, found {len(scan_paths)} frames")
    scan_states = [
        read_capture(path, "result" if path == scan_paths[-1] else "scan")
        for path in scan_paths
    ]

    wallpaper_source = cv2.imread(str(wallpaper_path), cv2.IMREAD_COLOR)
    if wallpaper_source is None:
        raise FileNotFoundError(wallpaper_path)
    wallpaper = cover(wallpaper_source, CANVAS_WIDTH, CANVAS_HEIGHT)
    wallpaper = cv2.GaussianBlur(wallpaper, (0, 0), 12)
    cursor_source = cv2.imread(
        str(wallpaper_path.parent / "macos-arrow.png"), cv2.IMREAD_UNCHANGED
    )
    if cursor_source is None or cursor_source.shape[2] != 4:
        raise FileNotFoundError(wallpaper_path.parent / "macos-arrow.png")
    cursor_image = cv2.resize(
        cursor_source, (20, 29), interpolation=cv2.INTER_LANCZOS4
    )
    output_dir.mkdir(parents=True, exist_ok=True)

    cv2.imwrite(
        str(output_dir / "scan-results.png"),
        compose_static(wallpaper_source, images["overview"]),
        [cv2.IMWRITE_PNG_COMPRESSION, 9],
    )
    cv2.imwrite(
        str(output_dir / "disk-list.png"),
        compose_static(wallpaper_source, images["home"]),
        [cv2.IMWRITE_PNG_COMPRESSION, 9],
    )
    save_gif(
        scan_frames(images["home"], scan_states, wallpaper, cursor_image),
        output_dir / "local-scan.gif",
        disposal=1,
    )
    save_gif(
        delete_frames(images["review"], images["queued"], wallpaper, cursor_image),
        output_dir / "drag-remove.gif",
        disposal=2,
    )

    all_paths = [raw_dir / filename for filename in BASE_FILES.values()] + scan_paths
    manifest = {
        "capture_set": asset_root.name,
        "declared_app_version": "0.6.1",
        "scan_capture_count": len(scan_paths),
        "raw_files": {path.name: sha256(path) for path in all_paths},
        "outputs": {
            name: sha256(output_dir / name)
            for name in (
                "scan-results.png",
                "disk-list.png",
                "local-scan.gif",
                "drag-remove.gif",
            )
        },
    }
    (asset_root / "website-media-manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("asset_root", type=Path)
    parser.add_argument("output_dir", type=Path)
    parser.add_argument("wallpaper", type=Path)
    args = parser.parse_args()
    render(args.asset_root, args.output_dir, args.wallpaper)


if __name__ == "__main__":
    main()
