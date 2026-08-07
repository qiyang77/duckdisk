#!/usr/bin/env python3
"""Render the wallpaper-backed DuckDisk App Store preview.

All UI states come from real Store-build window captures. The renderer adds a
real macOS wallpaper, restrained captions, a correctly mapped cursor, and the
between-state motion needed for a concise product story.
"""

from __future__ import annotations

import argparse
import math
from pathlib import Path

import cv2
import numpy as np
from PIL import Image, ImageDraw, ImageFont


FPS = 30
WIDTH = 1920
HEIGHT = 1080
DURATION = 29.0
APP_WIDTH = 1368
APP_HEIGHT = 866
APP_X = (WIDTH - APP_WIDTH) // 2
APP_Y = 70
SF_FONT = "/System/Library/Fonts/SFNS.ttf"


def clamp(value: float, low: float = 0.0, high: float = 1.0) -> float:
    return max(low, min(high, value))


def smoothstep(value: float) -> float:
    value = clamp(value)
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
    return resized[y:y + height, x:x + width]


def remove_capture_cursor(image: np.ndarray, center: tuple[int, int]) -> np.ndarray:
    # The Computer Use capture cursor includes a wide glow. Inpainting can
    # leave a visible swirl on DuckDisk's flat panes, so clone a neighboring
    # patch from the same horizontal band and feather it into place.
    result = image.copy()
    radius = 52
    x, y = center
    source_x = x - 150 if x >= image.shape[1] // 2 else x + 150
    x0, x1 = x - radius, x + radius
    y0, y1 = y - radius, y + radius
    sx0, sx1 = source_x - radius, source_x + radius
    patch = image[y0:y1, sx0:sx1].copy()
    target = result[y0:y1, x0:x1]
    mask = np.zeros((radius * 2, radius * 2), dtype=np.uint8)
    cv2.circle(mask, (radius, radius), radius - 5, 255, -1, cv2.LINE_AA)
    mask = cv2.GaussianBlur(mask, (0, 0), 7)
    alpha = (mask.astype(np.float32) / 255.0)[..., None]
    result[y0:y1, x0:x1] = (
        patch.astype(np.float32) * alpha
        + target.astype(np.float32) * (1.0 - alpha)
    ).astype(np.uint8)
    return result


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


def camera_parameters(
    image: np.ndarray, zoom: float, focus: tuple[float, float]
) -> tuple[float, float, float, float]:
    height, width = image.shape[:2]
    crop_width = width / zoom
    crop_height = height / zoom
    focus_x = focus[0] * width
    focus_y = focus[1] * height
    left = max(0.0, min(width - crop_width, focus_x - crop_width / 2.0))
    top = max(0.0, min(height - crop_height, focus_y - crop_height / 2.0))
    return left, top, crop_width, crop_height


def camera_view(
    image: np.ndarray, zoom: float, focus: tuple[float, float]
) -> np.ndarray:
    left, top, crop_width, crop_height = camera_parameters(image, zoom, focus)
    x0, y0 = round(left), round(top)
    x1, y1 = round(left + crop_width), round(top + crop_height)
    return cv2.resize(
        image[y0:y1, x0:x1],
        (image.shape[1], image.shape[0]),
        interpolation=cv2.INTER_LANCZOS4,
    )


def source_to_screen(
    image: np.ndarray,
    source_point: tuple[float, float],
    zoom: float,
    focus: tuple[float, float],
) -> tuple[float, float]:
    left, top, crop_width, crop_height = camera_parameters(image, zoom, focus)
    x = APP_X + ((source_point[0] - left) / crop_width) * APP_WIDTH
    y = APP_Y + ((source_point[1] - top) / crop_height) * APP_HEIGHT
    return x, y


def compose_window(
    wallpaper: np.ndarray,
    source: np.ndarray,
    zoom: float,
    focus: tuple[float, float],
) -> np.ndarray:
    frame = wallpaper.copy()

    shadow = np.zeros((HEIGHT, WIDTH), dtype=np.uint8)
    cv2.rectangle(
        shadow,
        (APP_X - 10, APP_Y + 14),
        (APP_X + APP_WIDTH + 10, APP_Y + APP_HEIGHT + 24),
        175,
        -1,
    )
    shadow = cv2.GaussianBlur(shadow, (0, 0), 30)
    alpha_shadow = (shadow.astype(np.float32) / 255.0 * 0.62)[..., None]
    frame = (frame.astype(np.float32) * (1.0 - alpha_shadow)).astype(np.uint8)

    viewed = camera_view(source, zoom, focus)
    window = cv2.resize(
        viewed, (APP_WIDTH, APP_HEIGHT), interpolation=cv2.INTER_LANCZOS4
    )
    mask = rounded_mask(APP_WIDTH, APP_HEIGHT, 18)
    alpha = (mask.astype(np.float32) / 255.0)[..., None]
    target = frame[APP_Y:APP_Y + APP_HEIGHT, APP_X:APP_X + APP_WIDTH]
    frame[APP_Y:APP_Y + APP_HEIGHT, APP_X:APP_X + APP_WIDTH] = (
        window.astype(np.float32) * alpha
        + target.astype(np.float32) * (1.0 - alpha)
    ).astype(np.uint8)
    return frame


def state_at(
    states: list[tuple[float, np.ndarray]], time: float, transition: float = 0.34
) -> np.ndarray:
    active = states[0][1]
    for start, next_image in states[1:]:
        if time < start:
            break
        if time < start + transition:
            amount = smoothstep((time - start) / transition)
            return cv2.addWeighted(active, 1.0 - amount, next_image, amount, 0.0)
        active = next_image
    return active


def camera_at(time: float) -> tuple[float, tuple[float, float]]:
    # One restrained push-in while inspecting the largest local files.
    if 7.45 <= time <= 11.90:
        if time < 8.35:
            zoom = 1.0 + 0.06 * smoothstep((time - 7.45) / 0.90)
        elif time > 10.90:
            zoom = 1.0 + 0.06 * (1.0 - smoothstep((time - 10.90) / 1.00))
        else:
            zoom = 1.06
        return zoom, (0.28, 0.70)
    return 1.0, (0.5, 0.5)


def interpolate_source_cursor(
    keyframes: list[tuple[float, tuple[float, float]]], time: float
) -> tuple[float, float]:
    if time <= keyframes[0][0]:
        return keyframes[0][1]
    if time >= keyframes[-1][0]:
        return keyframes[-1][1]
    for index in range(len(keyframes) - 1):
        t0, p0 = keyframes[index]
        t1, p1 = keyframes[index + 1]
        if t0 <= time <= t1:
            span = max(0.001, t1 - t0)
            u = smoothstep((time - t0) / span)
            dx, dy = p1[0] - p0[0], p1[1] - p0[1]
            distance = math.hypot(dx, dy)
            arc = min(24.0, distance * 0.045) * math.sin(math.pi * u)
            if distance > 0.001:
                nx, ny = -dy / distance, dx / distance
            else:
                nx, ny = 0.0, 0.0
            return p0[0] + dx * u + nx * arc, p0[1] + dy * u + ny * arc
    return keyframes[-1][1]


def draw_cursor(frame: np.ndarray, point: tuple[float, float]) -> None:
    x, y = int(round(point[0])), int(round(point[1]))
    polygon = np.array(
        [
            [x, y],
            [x + 2, y + 29],
            [x + 9, y + 22],
            [x + 17, y + 38],
            [x + 24, y + 34],
            [x + 16, y + 19],
            [x + 28, y + 18],
        ],
        dtype=np.int32,
    )
    shadow = polygon + np.array([3, 4], dtype=np.int32)
    overlay = frame.copy()
    cv2.fillPoly(overlay, [shadow], (0, 0, 0), cv2.LINE_AA)
    cv2.addWeighted(overlay, 0.48, frame, 0.52, 0.0, frame)
    cv2.fillPoly(frame, [polygon], (250, 250, 250), cv2.LINE_AA)
    cv2.polylines(frame, [polygon], True, (17, 17, 17), 2, cv2.LINE_AA)


def render_text_card(text: str, font_size: int = 29) -> np.ndarray:
    font = ImageFont.truetype(SF_FONT, font_size)
    probe = Image.new("RGBA", (1, 1))
    draw = ImageDraw.Draw(probe)
    box = draw.textbbox((0, 0), text, font=font)
    text_width = box[2] - box[0]
    text_height = box[3] - box[1]
    padding_x, padding_y = 25, 14
    width = text_width + padding_x * 2
    height = text_height + padding_y * 2
    image = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    draw.rounded_rectangle(
        (0, 0, width - 1, height - 1),
        radius=12,
        fill=(28, 31, 34, 212),
        outline=(255, 255, 255, 32),
        width=1,
    )
    draw.text(
        (padding_x, padding_y - box[1]),
        text,
        font=font,
        fill=(248, 248, 248, 255),
    )
    rgba = np.array(image)
    return cv2.cvtColor(rgba, cv2.COLOR_RGBA2BGRA)


def overlay_rgba(
    frame: np.ndarray, image: np.ndarray, x: int, y: int, opacity: float = 1.0
) -> None:
    height, width = image.shape[:2]
    if x >= frame.shape[1] or y >= frame.shape[0] or x + width <= 0 or y + height <= 0:
        return
    x0, y0 = max(0, x), max(0, y)
    x1, y1 = min(frame.shape[1], x + width), min(frame.shape[0], y + height)
    source = image[y0 - y:y1 - y, x0 - x:x1 - x]
    alpha = source[..., 3:4].astype(np.float32) / 255.0 * opacity
    target = frame[y0:y1, x0:x1].astype(np.float32)
    frame[y0:y1, x0:x1] = (
        source[..., :3].astype(np.float32) * alpha + target * (1.0 - alpha)
    ).astype(np.uint8)


def caption_alpha(time: float, start: float, end: float) -> float:
    fade = 0.32
    if time < start or time > end:
        return 0.0
    if time < start + fade:
        return smoothstep((time - start) / fade)
    if time > end - fade:
        return 1.0 - smoothstep((time - (end - fade)) / fade)
    return 1.0


def draw_captions(
    frame: np.ndarray,
    time: float,
    captions: list[tuple[float, float, np.ndarray]],
) -> None:
    for start, end, card in captions:
        alpha = caption_alpha(time, start, end)
        if alpha <= 0.0:
            continue
        x = (WIDTH - card.shape[1]) // 2
        y = 979
        overlay_rgba(frame, card, x, y, alpha)


def drag_chip() -> np.ndarray:
    font = ImageFont.truetype(SF_FONT, 23)
    text = "Old Screen Recording.mov"
    probe = Image.new("RGBA", (1, 1))
    draw = ImageDraw.Draw(probe)
    box = draw.textbbox((0, 0), text, font=font)
    width = box[2] - box[0] + 34
    height = box[3] - box[1] + 22
    image = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    draw.rounded_rectangle(
        (0, 0, width - 1, height - 1),
        radius=9,
        fill=(71, 22, 28, 235),
        outline=(245, 116, 126, 225),
        width=2,
    )
    draw.text((17, 11 - box[1]), text, font=font, fill=(255, 241, 242, 255))
    rgba = np.array(image)
    return cv2.cvtColor(rgba, cv2.COLOR_RGBA2BGRA)


def render(asset_root: Path, wallpaper_path: Path) -> Path:
    raw = asset_root / "raw"
    sources = {
        "home": raw / "01-home.jpeg",
        "results": raw / "02-local-scan-results.jpeg",
        "videos": raw / "03-videos-drill.jpeg",
        "review": raw / "04-before-drag-delete.jpeg",
        "queued": raw / "05-after-drag-delete.jpeg",
        "confirm": raw / "06-delete-confirmation.jpeg",
    }
    images: dict[str, np.ndarray] = {}
    for key, path in sources.items():
        image = cv2.imread(str(path), cv2.IMREAD_COLOR)
        if image is None:
            raise FileNotFoundError(path)
        if key == "home":
            center = (600, 600)
        elif key == "confirm":
            center = (220, 560)
        else:
            center = (600, 560)
        images[key] = remove_capture_cursor(image, center)

    wallpaper_source = cv2.imread(str(wallpaper_path), cv2.IMREAD_COLOR)
    if wallpaper_source is None:
        raise FileNotFoundError(wallpaper_path)
    wallpaper = cover(wallpaper_source, WIDTH, HEIGHT)

    states = [
        (0.0, images["home"]),
        (2.72, images["results"]),
        (7.18, images["videos"]),
        (12.12, images["review"]),
        (16.72, images["queued"]),
        (21.02, images["confirm"]),
    ]
    cursor_keys = [
        (0.00, (-80.0, 650.0)),
        (0.72, (110.0, 520.0)),
        (1.75, (250.0, 216.0)),
        (2.48, (250.0, 216.0)),
        (3.25, (420.0, 430.0)),
        (5.55, (150.0, 700.0)),
        (6.85, (150.0, 700.0)),
        (7.65, (150.0, 700.0)),
        (9.05, (170.0, 245.0)),
        (10.35, (170.0, 245.0)),
        (11.35, (500.0, 700.0)),
        (12.75, (320.0, 470.0)),
        (14.15, (155.0, 244.0)),
        (14.78, (155.0, 244.0)),
        (16.58, (1018.0, 642.0)),
        (17.35, (1018.0, 642.0)),
        (19.58, (1098.0, 737.0)),
        (20.62, (1098.0, 737.0)),
        (21.65, (930.0, 560.0)),
        (23.45, (640.0, 454.0)),
        (27.60, (640.0, 454.0)),
        (29.00, (700.0, 500.0)),
    ]
    captions = [
        (0.65, 2.60, render_text_card("Scan any local folder in seconds")),
        (3.20, 6.70, render_text_card("See exactly what takes up space")),
        (7.72, 11.55, render_text_card("Drill down to the largest files")),
        (12.55, 16.60, render_text_card("Drag unwanted files to the delete list")),
        (17.12, 20.72, render_text_card("Collect files before taking action")),
        (21.48, 27.75, render_text_card("Review every item before permanent deletion")),
    ]
    chip = drag_chip()

    intermediate = asset_root / "DuckDisk-App-Preview-v2-intermediate.mp4"
    writer = cv2.VideoWriter(
        str(intermediate),
        cv2.VideoWriter_fourcc(*"mp4v"),
        FPS,
        (WIDTH, HEIGHT),
    )
    if not writer.isOpened():
        raise RuntimeError("could not initialize video writer")

    total_frames = round(DURATION * FPS)
    for frame_index in range(total_frames):
        time = frame_index / FPS
        source = state_at(states, time)
        zoom, focus = camera_at(time)
        frame = compose_window(wallpaper, source, zoom, focus)

        source_cursor = interpolate_source_cursor(cursor_keys, time)
        cursor = source_to_screen(source, source_cursor, zoom, focus)

        if 14.78 <= time <= 16.65:
            target_left = APP_X + (848 / 1200.0) * APP_WIDTH
            target_top = APP_Y + (620 / 760.0) * APP_HEIGHT
            target_right = APP_X + (1192 / 1200.0) * APP_WIDTH
            target_bottom = APP_Y + (662 / 760.0) * APP_HEIGHT
            overlay = frame.copy()
            cv2.rectangle(
                overlay,
                (round(target_left), round(target_top)),
                (round(target_right), round(target_bottom)),
                (82, 103, 232),
                3,
                cv2.LINE_AA,
            )
            cv2.addWeighted(overlay, 0.65, frame, 0.35, 0.0, frame)
            overlay_rgba(frame, chip, round(cursor[0] + 18), round(cursor[1] + 18), 0.96)

        draw_cursor(frame, cursor)
        draw_captions(frame, time, captions)

        fade = 1.0
        if time < 0.38:
            fade *= smoothstep(time / 0.38)
        if time > DURATION - 0.70:
            fade *= 1.0 - smoothstep((time - (DURATION - 0.70)) / 0.70)
        if fade < 0.999:
            frame = (frame.astype(np.float32) * fade).astype(np.uint8)
        writer.write(frame)

    writer.release()
    return intermediate


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("asset_root", type=Path)
    parser.add_argument("wallpaper", type=Path)
    args = parser.parse_args()
    print(render(args.asset_root, args.wallpaper))


if __name__ == "__main__":
    main()
