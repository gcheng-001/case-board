#!/usr/bin/env python3
from __future__ import annotations

import argparse
import concurrent.futures
import io
import datetime as _dt
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.error
import urllib.request
from typing import Iterable, List, Optional, Tuple


DEFAULT_OUT_BASE = Path("/Users/Apple/Desktop/录屏取证输出")
VISION_OCR_CLI = Path.home() / ".local" / "bin" / "vision-ocr-pdf"


def _die(message: str, code: int = 1) -> None:
    print(f"Error: {message}", file=sys.stderr)
    raise SystemExit(code)


def _ensure_bins(names: Iterable[str]) -> None:
    missing = [name for name in names if shutil.which(name) is None]
    if missing:
        _die("Missing command(s): " + ", ".join(missing))


def _run(cmd: List[str]) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, text=True, capture_output=True)


def _format_ts(seconds: float) -> str:
    total = int(round(seconds))
    h = total // 3600
    m = (total % 3600) // 60
    s = total % 60
    return f"{h:02d}:{m:02d}:{s:02d}"


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _unique_out_dir(out_base: Path, video_path: Path) -> Path:
    stamp = _dt.datetime.now().strftime("%Y%m%d_%H%M%S_%f")[:-3]
    safe_stem = video_path.stem.replace("/", "_")
    out_dir = out_base / f"{safe_stem}_{stamp}"
    # Extremely defensive collision avoidance.
    for i in range(1000):
        candidate = out_dir if i == 0 else Path(str(out_dir) + f"_{i}")
        if not candidate.exists():
            return candidate
    _die("Failed to allocate unique output directory (too many collisions).")
    raise AssertionError("unreachable")


def _interval_tag(interval_sec: float) -> str:
    s = f"{interval_sec}".rstrip("0").rstrip(".")
    s = s.replace(".", "p")
    return f"i{s}"


def _variant_label(interval_sec: float) -> str:
    s = f"{interval_sec}".rstrip("0").rstrip(".")
    s = s.replace(".", "p")
    return f"V{s}"


def _review_dir(out_dir: Path) -> Path:
    return out_dir / "复核资料"


def _final_frames_dir(out_dir: Path, label: str = "") -> Path:
    return out_dir / (f"截图_{label}" if label else "截图")


def _raw_frames_dir(out_dir: Path) -> Path:
    return _review_dir(out_dir) / "原始抽帧"


def _list_raw_frames(raw_dir: Path, image_ext: str) -> List[Path]:
    frames = sorted(raw_dir.glob(f"raw_*.{image_ext}"))
    if not frames:
        frames = sorted([p for p in raw_dir.glob("raw_*.*") if p.suffix.lower() in {".png", ".jpg", ".jpeg"}])
    return frames


def _pdf_path(out_dir: Path, label: str = "") -> Path:
    suffix = f"_{label}" if label else ""
    return out_dir / f"录屏取证初稿{suffix}.pdf"


def _frame_index_path(out_dir: Path, label: str = "") -> Path:
    suffix = f"_{label}" if label else ""
    return _review_dir(out_dir) / f"截图索引{suffix}.jsonl"


def _selected_index_path(out_dir: Path, label: str = "") -> Path:
    suffix = f"_{label}" if label else ""
    return _review_dir(out_dir) / f"筛选记录{suffix}.jsonl"


def _ocr_index_path(out_dir: Path) -> Path:
    return _review_dir(out_dir) / "OCR文字索引.jsonl"


def _ocr_markdown_path(out_dir: Path) -> Path:
    return out_dir / "录屏取证初稿_OCR文字索引.md"


def _cloud_markdown_path(out_dir: Path) -> Path:
    return out_dir / "云端增强_聊天线索.md"


def _analysis_report_path(out_dir: Path) -> Path:
    return out_dir / "聊天记录分析报告.md"


def _cloud_audit_path(out_dir: Path) -> Path:
    return _review_dir(out_dir) / "云端增强审计.json"


def _build_vf(interval_sec: float, max_width: int) -> str:
    vf = f"fps=1/{interval_sec}"
    if max_width > 0:
        # Keep aspect ratio; only shrink if wider than max_width.
        vf += f",scale='if(gt(iw,{max_width}),{max_width},iw)':-2:flags=lanczos"
    return vf


def _extract_frames(
    video_path: Path,
    frames_dir: Path,
    interval_sec: float,
    image_ext: str,
    start_sec: float,
    duration_sec: Optional[float],
    max_width: int,
) -> List[Path]:
    frames_dir.mkdir(parents=True, exist_ok=True)
    pattern = frames_dir / f"frame_%06d.{image_ext}"

    cmd: List[str] = ["ffmpeg", "-y", "-hide_banner", "-loglevel", "error"]
    cmd += ["-i", str(video_path)]
    if start_sec > 0:
        cmd += ["-ss", str(start_sec)]
    if duration_sec is not None and duration_sec > 0:
        cmd += ["-t", str(duration_sec)]
    cmd += ["-vf", _build_vf(interval_sec, max_width)]
    if image_ext in {"jpg", "jpeg"}:
        cmd += ["-q:v", "2"]
    cmd += [str(pattern)]

    res = _run(cmd)
    if res.returncode != 0:
        _die(f"ffmpeg failed:\n{res.stderr.strip()}")

    frames = sorted(frames_dir.glob(f"frame_*.{image_ext}"))
    if not frames:
        _die("No frames extracted. Check the input video and interval.")
    return frames


def _warn_if_huge(interval_sec: float, start_sec: float, duration_sec: Optional[float], max_frames: int) -> None:
    # Conservative heuristic: warn when user might create a giant export.
    if interval_sec <= 0:
        return
    if duration_sec is None or duration_sec <= 0:
        # Unknown total duration without ffprobe; avoid extra deps and just warn on tiny interval.
        if interval_sec < 1 and max_frames <= 0:
            print(
                "Warning: interval < 1s may create a huge number of frames for long videos. "
                "Consider --max-frames or --duration.",
                file=sys.stderr,
            )
        return
    estimated = int(duration_sec / interval_sec) + 2
    if max_frames > 0:
        estimated = min(estimated, max_frames)
    if estimated >= 4000:
        print(
            f"Warning: estimated frames ~= {estimated}. "
            "This may be slow and produce a very large PDF. "
            "Consider --interval 1/2, --max-frames, --duration, or --max-width.",
            file=sys.stderr,
        )


def _link_or_copy(src: Path, dst: Path) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    try:
        os.link(src, dst)
    except Exception:
        shutil.copy2(src, dst)


def _sharpness_score(image_path: Path) -> float:
    try:
        from PIL import Image, ImageFilter, ImageStat
    except ImportError:
        _die("Missing Python dependency: Pillow (PIL). Install it before running.")

    with Image.open(image_path) as im:
        edges = im.convert("L").filter(ImageFilter.FIND_EDGES)
        stat = ImageStat.Stat(edges)
        return float(stat.var[0])


def _dhash64(image_path: Path, hash_size: int = 8) -> int:
    try:
        from PIL import Image
    except ImportError:
        _die("Missing Python dependency: Pillow (PIL). Install it before running.")

    with Image.open(image_path) as im:
        gray = im.convert("L")
        resized = gray.resize((hash_size + 1, hash_size))
        if hasattr(resized, "get_flattened_data"):
            pixels = list(resized.get_flattened_data())  # Pillow >= 11
        else:
            pixels = list(resized.getdata())

    value = 0
    bit = 0
    for row in range(hash_size):
        row_start = row * (hash_size + 1)
        for col in range(hash_size):
            left = pixels[row_start + col]
            right = pixels[row_start + col + 1]
            if left > right:
                value |= 1 << bit
            bit += 1
    return value


def _hamming_distance(a: int, b: int) -> int:
    return bin(a ^ b).count('1')


def _visual_delta_score(left_path: Path, right_path: Path) -> float:
    try:
        from PIL import Image, ImageChops, ImageStat
    except ImportError:
        _die("Missing Python dependency: Pillow (PIL). Install it before running.")

    with Image.open(left_path) as left, Image.open(right_path) as right:
        left_im = left.convert("L").resize((96, 160))
        right_im = right.convert("L").resize((96, 160))
        diff = ImageChops.difference(left_im, right_im)
        stat = ImageStat.Stat(diff)
        return float(stat.rms[0])


def _thumb_bytes_for_dedupe(
    image_path: Path,
    *,
    size: int = 48,
    crop_top_ratio: float = 0.12,
    crop_bottom_ratio: float = 0.12,
) -> bytes:
    try:
        from PIL import Image
    except ImportError:
        _die("Missing Python dependency: Pillow (PIL). Install it before running.")

    with Image.open(image_path) as im:
        img = im.convert("L")
        w, h = img.size
        if w <= 0 or h <= 0:
            return b""
        top = int(max(0, min(h - 1, round(h * crop_top_ratio))))
        bottom_cut = int(max(0, min(h - 1, round(h * crop_bottom_ratio))))
        bottom = max(top + 1, h - bottom_cut)
        img = img.crop((0, top, w, bottom))
        img = img.resize((size, size))
        return img.tobytes()


def _mean_abs_diff_bytes(left: bytes, right: bytes) -> Optional[float]:
    if not left or not right or len(left) != len(right):
        return None
    total = 0
    for a, b in zip(left, right):
        total += a - b if a >= b else b - a
    return total / float(len(left))


def _build_vf_fps(fps: float, max_width: int) -> str:
    vf = f"fps={fps}"
    if max_width > 0:
        vf += f",scale='if(gt(iw,{max_width}),{max_width},iw)':-2:flags=lanczos"
    return vf


def _extract_frames_burst(
    video_path: Path,
    frames_dir: Path,
    burst_fps: float,
    image_ext: str,
    start_sec: float,
    duration_sec: Optional[float],
    max_width: int,
    max_raw_frames: Optional[int],
) -> List[Path]:
    frames_dir.mkdir(parents=True, exist_ok=True)
    pattern = frames_dir / f"raw_%06d.{image_ext}"

    cmd: List[str] = ["ffmpeg", "-y", "-hide_banner", "-loglevel", "error"]
    cmd += ["-i", str(video_path)]
    if start_sec > 0:
        cmd += ["-ss", str(start_sec)]
    if duration_sec is not None and duration_sec > 0:
        cmd += ["-t", str(duration_sec)]
    if max_raw_frames is not None and max_raw_frames > 0:
        cmd += ["-frames:v", str(max_raw_frames)]
    cmd += ["-vf", _build_vf_fps(burst_fps, max_width)]
    if image_ext in {"jpg", "jpeg"}:
        cmd += ["-q:v", "2"]
    cmd += [str(pattern)]

    res = _run(cmd)
    if res.returncode != 0:
        _die(f"ffmpeg burst extraction failed:\n{res.stderr.strip()}")

    frames = sorted(frames_dir.glob(f"raw_*.{image_ext}"))
    if not frames:
        _die("No raw frames extracted. Check the input video and parameters.")
    return frames


def _image_as_pdf_jpeg_stream(image_path: Path, jpeg_quality: int) -> Tuple[bytes, int, int]:
    try:
        from PIL import Image, ImageOps
    except ImportError:
        _die("Missing Python dependency: Pillow (PIL). Install it before running.")

    with Image.open(image_path) as im:
        im = ImageOps.exif_transpose(im)
        if im.mode not in {"RGB", "L"}:
            bg = Image.new("RGB", im.size, "white")
            if im.mode == "RGBA":
                bg.paste(im, mask=im.getchannel("A"))
            else:
                bg.paste(im.convert("RGB"))
            im = bg
        elif im.mode == "L":
            im = im.convert("RGB")
        else:
            im = im.convert("RGB")
        width, height = im.size
        buf = io.BytesIO()
        im.save(buf, format="JPEG", quality=jpeg_quality, optimize=True, progressive=False)
    return buf.getvalue(), width, height


def _pdf_add_image(doc: "fitz.Document", image_path: Path, jpeg_quality: int) -> Tuple[int, int]:
    stream, width, height = _image_as_pdf_jpeg_stream(image_path, jpeg_quality=jpeg_quality)

    # 1px == 1pt to avoid any resampling surprises; page size matches screenshot size.
    page = doc.new_page(width=width, height=height)
    rect = page.rect
    page.insert_image(rect, stream=stream, keep_proportion=False)
    return width, height


def _build_pdf_streaming(frames: List[Path], pdf_path: Path, jpeg_quality: int = 88) -> None:
    try:
        import fitz  # PyMuPDF
    except ImportError:
        _die("Missing Python dependency: PyMuPDF (fitz). Install it before running.")

    jpeg_quality = max(60, min(95, int(jpeg_quality)))
    pdf_path.parent.mkdir(parents=True, exist_ok=True)
    doc = fitz.open()
    try:
        for frame in frames:
            _pdf_add_image(doc, frame, jpeg_quality=jpeg_quality)
        doc.save(str(pdf_path), garbage=4, deflate=True)
    finally:
        doc.close()


def _write_index_jsonl(
    frames: List[Path],
    index_path: Path,
    interval_sec: float,
    start_sec: float,
    include_sha256: bool = True,
) -> None:
    index_path.parent.mkdir(parents=True, exist_ok=True)
    with index_path.open("w", encoding="utf-8") as fp:
        for idx, frame_path in enumerate(frames, start=1):
            ts_sec = start_sec + (idx - 1) * interval_sec
            record = {
                "frame": frame_path.name,
                "frame_path": str(frame_path),
                "timestamp_sec": round(ts_sec, 3),
                "timestamp_hms": _format_ts(ts_sec),
            }
            if include_sha256:
                record["sha256"] = _sha256_file(frame_path)
            fp.write(json.dumps(record, ensure_ascii=False) + "\n")


def _read_jsonl(path: Path) -> List[dict]:
    if not path.exists():
        _die(f"Missing index file: {path}")
    records: List[dict] = []
    with path.open("r", encoding="utf-8") as fp:
        for line in fp:
            line = line.strip()
            if line:
                records.append(json.loads(line))
    return records


def _validate_export_dir(out_dir: Path, require_ocr: bool = False) -> List[str]:
    missing: List[str] = []
    if not out_dir.exists():
        return [f"输出目录不存在: {out_dir}"]
    pdf_path = _pdf_path(out_dir)
    if not pdf_path.exists():
        missing.append(f"缺少PDF: {pdf_path.name}")
    frames_dir = _final_frames_dir(out_dir)
    if not frames_dir.exists() or not any(frames_dir.iterdir()):
        missing.append("缺少最终截图目录或截图为空")
    frame_index = _frame_index_path(out_dir)
    if not frame_index.exists():
        missing.append(f"缺少截图索引: {frame_index.name}")
    if require_ocr:
        ocr_index = _ocr_index_path(out_dir)
        ocr_md = _ocr_markdown_path(out_dir)
        if not ocr_index.exists():
            missing.append(f"缺少OCR索引: {ocr_index.name}")
        if not ocr_md.exists():
            missing.append(f"缺少OCR Markdown: {ocr_md.name}")
    return missing


def _normalize_text(text: str) -> str:
    lines = [line.rstrip() for line in text.replace("\r\n", "\n").replace("\r", "\n").split("\n")]
    cleaned = [line for line in lines if line.strip()]
    return "\n".join(cleaned).strip()


def _clean_ocr_text(text: str, frame_stem: str) -> str:
    cleaned: List[str] = []
    for line in _normalize_text(text).splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        if stripped == f"# {frame_stem}" or stripped.startswith("## 第"):
            continue
        cleaned.append(stripped)
    return "\n".join(cleaned).strip()


def _strip_markdown_emphasis(text: str) -> str:
    stripped = text.strip()
    while stripped.startswith("*") and stripped.endswith("*") and len(stripped) >= 2:
        stripped = stripped[1:-1].strip()
    return stripped


_FULL_CHAT_TIME_RE = re.compile(r"^\s*(\d{4}年\d{1,2}月\d{1,2}日\s+\d{1,2}:\d{2})\s*$")
_ANY_CHAT_TIME_RE = re.compile(r"(\d{4}年\d{1,2}月\d{1,2}日\s*)?\d{1,2}:\d{2}")
_NOISE_EXACT = {
    "<",
    ">",
    "+",
    "©",
    "③",
    "⑤ +",
    "•••",
    "••。",
    "=•。",
    "=••",
    "朋友圈",
    "发消息",
    "音视频通话",
    "朋友资料",
    "电话",
    "地区",
    "昵称",
}
_NOISE_KEYWORDS = (
    "中国电信",
    "专注模式",
    "未在播放",
    "微信号：",
    "昵称：",
    "地区：",
    "朋友使用的铃声",
)


def _normalise_message_key(text: str) -> str:
    text = _strip_markdown_emphasis(text)
    text = re.sub(r"\s+", "", text)
    text = re.sub(r"[，。！？、；：:,.!?;\"'“”‘’（）()【】\\[\\]<>《》\-_=+•·*#|/\\\\]", "", text)
    return text.lower()


def _is_noise_line(line: str) -> bool:
    clean = _strip_markdown_emphasis(line).strip()
    compact = re.sub(r"\s+", "", clean)
    if not clean:
        return True
    if clean in _NOISE_EXACT or compact in _NOISE_EXACT:
        return True
    if any(keyword in clean for keyword in _NOISE_KEYWORDS):
        return True
    if re.fullmatch(r"[oO0lLI1|！!:.。•·＊*()（）\[\]【】]+", compact):
        return True
    if re.fullmatch(r"[①②③④⑤⑥⑦⑧⑨⑩]?[•·]?\d{1,2}\"[（(]?", compact):
        return True
    return False


def _clean_chat_lines(text: str) -> List[str]:
    lines = []
    for raw in text.splitlines():
        line = _strip_markdown_emphasis(raw).strip().strip("| ")
        line = re.sub(r"\s+", " ", line).strip()
        if _is_noise_line(line):
            continue
        lines.append(line)
    return lines


def _infer_speaker(content: str) -> str:
    compact = _normalise_message_key(content)
    if not compact:
        return "需人工确认"
    lines = [line.strip() for line in content.splitlines() if line.strip()]
    if "撤回了一条消息" in content or "你已添加" in content or "以上是打招呼的内容" in content:
        return "系统/微信"
    if lines and "通话时长" in lines[0] and len(lines) <= 2:
        return "系统/微信"
    return "需人工确认"


def _flush_chat_message(
    messages: List[dict],
    record: dict,
    chat_time: str,
    lines: List[str],
    seen_message_keys: set,
    seen_line_keys: set,
) -> None:
    cleaned_lines = [line for line in lines if not _is_noise_line(line)]
    if not cleaned_lines:
        return

    new_lines = []
    local_line_keys = set()
    skipped_lines = 0
    for line in cleaned_lines:
        key = _normalise_message_key(line)
        if len(key) >= 4 and (key in seen_line_keys or key in local_line_keys):
            skipped_lines += 1
            continue
        new_lines.append(line)
        if len(key) >= 4:
            local_line_keys.add(key)

    if not new_lines:
        return

    content = "\n".join(new_lines).strip()
    message_key = f"{chat_time}|{_normalise_message_key(content)}"
    if len(message_key) >= 8 and message_key in seen_message_keys:
        return

    for line in new_lines:
        key = _normalise_message_key(line)
        if len(key) >= 4:
            seen_line_keys.add(key)
    seen_message_keys.add(message_key)

    messages.append(
        {
            "chat_time": chat_time or "需人工确认",
            "speaker": _infer_speaker(content),
            "content": content,
            "frame": record.get("frame", ""),
            "timestamp_hms": record.get("timestamp_hms", ""),
            "frame_path": record.get("frame_path", ""),
            "sha256": record.get("sha256", ""),
            "dedupe_note": f"跳过重复行 {skipped_lines} 条" if skipped_lines else "",
        }
    )


def _extract_structured_chat_messages(records: List[dict]) -> List[dict]:
    messages: List[dict] = []
    seen_message_keys: set = set()
    seen_line_keys: set = set()
    for record in records:
        lines = _clean_chat_lines(record.get("chat_content") or record.get("text") or "")
        current_time = ""
        buffer: List[str] = []
        for line in lines:
            full_time = _FULL_CHAT_TIME_RE.match(line)
            if full_time:
                _flush_chat_message(messages, record, current_time, buffer, seen_message_keys, seen_line_keys)
                current_time = full_time.group(1)
                buffer = []
                continue
            if current_time:
                buffer.append(line)
        _flush_chat_message(messages, record, current_time, buffer, seen_message_keys, seen_line_keys)
    return messages


def _markdown_cell(text: str, limit: int = 300) -> str:
    text = re.sub(r"\s+", "<br>", str(text or "").strip())
    if len(text) > limit:
        text = text[:limit].rstrip() + "..."
    return text.replace("|", "｜")


def _extract_chat_fields(text: str) -> Tuple[str, str]:
    lines = [_strip_markdown_emphasis(line) for line in text.splitlines() if line.strip()]
    chat_time = ""
    content_start = 0
    for idx, line in enumerate(lines):
        if _ANY_CHAT_TIME_RE.search(line):
            chat_time = line
            content_start = idx + 1
            break
    content_lines = lines[content_start:] if chat_time else lines
    chat_content = "\n".join(content_lines).strip()
    return chat_time, chat_content


def _ocr_text_score(text: str) -> int:
    score = 0
    for ch in text:
        if "\u4e00" <= ch <= "\u9fff":
            score += 2
        elif ch.isalnum():
            score += 1
    return score


def _scaled_ocr_image(src: Path, dst: Path) -> None:
    try:
        from PIL import Image, ImageOps
    except ImportError:
        _die("Missing Python dependency: Pillow (PIL). Install it before running.")

    with Image.open(src) as im:
        im = ImageOps.exif_transpose(im)
        max_side = max(im.size)
        scale = 2.0 if max_side <= 3200 else max(1.0, min(1.6, 6400 / max_side))
        if scale <= 1.05:
            im.save(dst)
            return
        new_size = (int(im.width * scale), int(im.height * scale))
        im.resize(new_size, Image.Resampling.LANCZOS).save(dst)


def _records_for_ocr(out_dir: Path, scope: str) -> List[dict]:
    if scope == "selected":
        return _read_jsonl(_frame_index_path(out_dir))
    if scope == "raw":
        records = _read_jsonl(_review_dir(out_dir) / "原始抽帧索引.jsonl")
        for record in records:
            frame_path = Path(record["frame_path"])
            if frame_path.exists() and "sha256" not in record:
                record["sha256"] = _sha256_file(frame_path)
        return records
    _die(f"Unsupported OCR scope: {scope}")
    raise AssertionError("unreachable")


def _cached_ocr_records(out_dir: Path) -> dict[Tuple[str, str], dict]:
    cached: dict[Tuple[str, str], dict] = {}
    index_path = _ocr_index_path(out_dir)
    if not index_path.exists():
        return cached
    for record in _read_jsonl(index_path):
        frame = str(record.get("frame") or "")
        sha = str(record.get("sha256") or "")
        if frame and sha and ("text" in record or record.get("status")):
            cached[(frame, sha)] = record
    return cached


def _ocr_single_record(record: dict) -> dict:
    frame_path = Path(record["frame_path"])
    if not frame_path.exists():
        return {
            **record,
            "status": "missing_frame",
            "text": "",
            "error": f"截图不存在: {frame_path}",
        }

    best_text = ""
    best_error = ""
    with tempfile.TemporaryDirectory(prefix="wechat-evidence-ocr-") as tmp_dir:
        tmp_root = Path(tmp_dir)
        scaled_path = tmp_root / f"{frame_path.stem}_scaled.png"
        candidates = [(frame_path, "original")]
        try:
            _scaled_ocr_image(frame_path, scaled_path)
            if scaled_path.exists():
                candidates.append((scaled_path, "scaled"))
        except Exception as exc:
            best_error = f"预处理失败: {exc}"

        for candidate_path, source in candidates:
            out_path = tmp_root / f"{source}.md"
            cmd = [
                str(VISION_OCR_CLI),
                str(candidate_path),
                "--output",
                str(out_path),
                "--type",
                "wechat",
                "--layout",
                "plain",
                "--recognition-level",
                "accurate",
                "--engine",
                "ocr",
            ]
            proc = _run(cmd)
            if proc.returncode != 0:
                best_error = proc.stderr.strip() or proc.stdout.strip() or "vision-ocr-pdf failed"
                continue
            raw_text = out_path.read_text(encoding="utf-8") if out_path.exists() else ""
            text = _clean_ocr_text(raw_text, candidate_path.stem)
            if _ocr_text_score(text) > _ocr_text_score(best_text):
                best_text = text
            if _ocr_text_score(best_text) >= 18:
                break

    return {
        **record,
        "status": "ok" if best_text else "empty",
        "text": best_text,
        "chat_time": _extract_chat_fields(best_text)[0],
        "chat_content": _extract_chat_fields(best_text)[1],
        "error": "" if best_text else best_error,
    }


def _write_ocr_outputs(out_dir: Path, records: List[dict], scope: str) -> Tuple[Path, Path]:
    index_path = _ocr_index_path(out_dir)
    markdown_path = _ocr_markdown_path(out_dir)
    index_path.parent.mkdir(parents=True, exist_ok=True)
    with index_path.open("w", encoding="utf-8") as fp:
        for record in records:
            fp.write(json.dumps(record, ensure_ascii=False) + "\n")

    messages = _extract_structured_chat_messages(records)
    lines = [
        "# 微信聊天记录转写 MD",
        "",
        "需人工核实。本文件根据录屏截图 OCR 自动整理，发言人仅在能稳定判断时标注；不能可靠判断时写“需人工确认”。",
        "",
        f"- 识别范围：{'入选截图' if scope == 'selected' else '原始抽帧'}",
        f"- OCR 截图数：{len(records)}",
        f"- 去重后聊天条目：{len(messages)}",
        "- 去重规则：相邻截图中已经出现过的相同话语不重复列入；保留来源截图用于人工复核。",
        "",
        "## 结构化聊天记录（去重）",
        "",
        "| 序号 | 聊天时间 | 发言人 | 内容 | 来源截图 | 录屏时间 | 处理说明 |",
        "| --- | --- | --- | --- | --- | --- | --- |",
    ]
    if messages:
        for idx, message in enumerate(messages, start=1):
            lines.append(
                "| {idx} | {chat_time} | {speaker} | {content} | {frame} | {screen_time} | {note} |".format(
                    idx=idx,
                    chat_time=_markdown_cell(message.get("chat_time", ""), 80),
                    speaker=_markdown_cell(message.get("speaker", ""), 80),
                    content=_markdown_cell(message.get("content", ""), 500),
                    frame=_markdown_cell(message.get("frame", ""), 80),
                    screen_time=_markdown_cell(message.get("timestamp_hms", ""), 80),
                    note=_markdown_cell(message.get("dedupe_note", ""), 120),
                )
            )
    else:
        lines.append("| 1 | 需人工确认 | 需人工确认 | 未能从 OCR 中稳定抽取聊天条目 |  |  |  |")

    lines.extend(["", "## 按截图复核 OCR", ""])
    for idx, record in enumerate(records, start=1):
        chat_time = record.get("chat_time", "")
        chat_content = "\n".join(_clean_chat_lines(record.get("chat_content", "") or record.get("text", "")))
        lines.extend(
            [
                f"## {idx}. {record.get('frame', '')}",
                "",
                f"- 录屏时间：{record.get('timestamp_hms', '')}",
                f"- 聊天时间：{chat_time or '（未识别）'}",
                f"- 截图：`{record.get('frame_path', '')}`",
                f"- SHA256：`{record.get('sha256', '')}`",
                f"- 状态：`{record.get('status', '')}`",
            ]
        )
        if record.get("error"):
            lines.append(f"- 错误：`{record['error']}`")
        lines.extend(["", "### 聊天内容", "", chat_content or "（无识别文字）", ""])
    markdown_path.write_text("\n".join(lines).rstrip() + "\n", encoding="utf-8")
    return index_path, markdown_path


def _speaker_counts(records: List[dict]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for message in _extract_structured_chat_messages(records):
        speaker = message.get("speaker", "")
        if not speaker or speaker == "需人工确认":
            continue
        counts[speaker] = counts.get(speaker, 0) + 1
    return counts


def _amount_candidates(records: List[dict]) -> List[str]:
    found: List[str] = []
    patterns = [
        r"(?:￥|¥)\s*\d[\d,]*(?:\.\d+)?",
        r"\d[\d,]*(?:\.\d+)?\s*元",
        r"\d[\d,]*(?:\.\d+)?\s*万",
    ]
    for record in records:
        text = record.get("chat_content", "") or record.get("text", "")
        for pattern in patterns:
            for match in re.findall(pattern, text):
                value = match.strip()
                if value not in found:
                    found.append(value)
    return found[:10]


def _event_summary(record: dict) -> str:
    content = record.get("chat_content", "") or record.get("text", "")
    lines = [_strip_markdown_emphasis(line) for line in content.splitlines() if line.strip()]
    if len(lines) >= 2 and len(lines[0]) <= 12:
        lines = lines[1:]
    summary = "；".join(lines).strip()
    return summary[:160] if summary else "（无识别内容）"


def _write_local_analysis_report(out_dir: Path, records: List[dict]) -> Path:
    report_path = _analysis_report_path(out_dir)
    messages = _extract_structured_chat_messages(records)
    speakers = _speaker_counts(records)
    amounts = _amount_candidates(records)
    speaker_text = "、".join(f"{name}（{count}条）" for name, count in sorted(speakers.items(), key=lambda item: -item[1]))
    amount_text = "、".join(amounts)

    lines = [
        "# 聊天记录分析报告",
        "",
        "需人工核实。本报告基于录屏截图 OCR 自动生成，仅用于律师内部梳理，不替代原始录屏、截图及人工判断。",
        "",
        "## 一、当事人 / 聊天双方",
        "",
        f"- 自动识别候选：{speaker_text or '（未能稳定识别，需人工补充）'}",
        "- 原告：需人工确认",
        "- 被告：需人工确认",
        "",
        "## 二、涉案金额",
        "",
        f"- OCR 金额候选：{amount_text or '（未识别到明确金额）'}",
        "- 最终金额口径：需人工确认",
        "",
        "## 三、时间线",
        "",
        "| 序号 | 聊天时间 | 发言人 | 事件 / 内容 | 对应截图 | 录屏时间 |",
        "| --- | --- | --- | --- | --- | --- |",
    ]
    for idx, message in enumerate(messages, start=1):
        lines.append(
            "| {idx} | {chat_time} | {speaker} | {event} | {frame} | {screen_time} |".format(
                idx=idx,
                chat_time=_markdown_cell(message.get("chat_time", ""), 80),
                speaker=_markdown_cell(message.get("speaker", ""), 80),
                event=_markdown_cell(message.get("content", ""), 500),
                frame=_markdown_cell(message.get("frame", ""), 80),
                screen_time=_markdown_cell(message.get("timestamp_hms", ""), 80),
            )
        )
    if not messages:
        lines.append("| 1 | 需人工确认 | 需人工确认 | 未能从 OCR 中稳定抽取聊天条目 |  |  |")

    lines.extend(
        [
            "",
            "## 四、可能有证据价值的材料",
            "",
            "- 涉及承诺、催款、还款时间、账户提供、金额确认、身份确认的聊天页，应优先人工复核并保留对应截图。",
            "- 对同一事实连续出现的聊天页，应结合前后文判断是否形成完整证据链。",
            "",
            "## 五、待人工核实事项",
            "",
            "- 聊天双方真实身份及微信昵称对应关系。",
            "- OCR 识别的聊天时间、金额、姓名、账号是否与原图一致。",
            "- 是否存在录屏前后未截入 PDF 的关键上下文。",
            "- 拟提交法院前，应从 PDF 中筛选必要页面，另行制作正式证据材料。",
            "",
            "## 六、截图索引",
            "",
        ]
    )
    for record in records:
        lines.append(f"- {record.get('frame', '')}: `{record.get('frame_path', '')}`")
    report_path.write_text("\n".join(lines).rstrip() + "\n", encoding="utf-8")
    return report_path


def _call_cloud_text_summary(
    records: List[dict],
    base_url: str,
    model: str,
    api_key: str,
    timeout_sec: float,
) -> dict:
    payload_records = [
        {
            "frame": record.get("frame", ""),
            "timestamp_hms": record.get("timestamp_hms", ""),
            "chat_time": record.get("chat_time", ""),
            "sha256": record.get("sha256", ""),
            "chat_content": record.get("chat_content", "") or record.get("text", ""),
        }
        for record in records
        if record.get("text")
    ]
    prompt = {
        "role": "user",
        "content": (
            "以下是微信聊天录屏截图的标准 OCR 索引，仅供律师内部整理线索，需人工核实。"
            "请生成一份正式的 Markdown《聊天记录分析报告》。必须使用以下结构："
            "一、当事人 / 聊天双方；二、涉案金额；三、时间线；四、关键聊天内容；"
            "五、与案件事实有关或容易被忽略的材料；六、待人工核实事项；七、截图索引。"
            "时间线必须用表格，列为：时间、事件、对应截图、核实状态。"
            "无法从 OCR 确认的内容必须写“需人工确认”，不得编造。"
            "不得把 OCR 猜测写成已证实事实。禁止生成报告日期、当前日期或 OCR 中没有出现的日期。"
            "每个关键片段尽量引用录屏时间、聊天时间和 frame 编号。\n\n"
            + json.dumps(payload_records, ensure_ascii=False)
        ),
    }
    body = json.dumps(
        {
            "model": model,
            "messages": [
                {
                    "role": "system",
                    "content": "你是律师取证整理助手。输出必须包含“需人工核实”提示，不得把OCR内容当成事实定论。只能使用OCR索引中的事实，不得补写当前日期、报告日期或外部事实。",
                },
                prompt,
            ],
            "temperature": 0.2,
        },
        ensure_ascii=False,
    ).encode("utf-8")
    endpoint = base_url.rstrip("/") + "/chat/completions"
    req = urllib.request.Request(
        endpoint,
        data=body,
        headers={
            "Content-Type": "application/json",
            "Authorization": f"Bearer {api_key}",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout_sec) as resp:
            raw = resp.read()
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", errors="replace")
        _die(f"Cloud summary request failed: HTTP {exc.code}: {detail}")
    except urllib.error.URLError as exc:
        _die(f"Cloud summary request failed: {exc}")

    data = json.loads(raw.decode("utf-8"))
    choices = data.get("choices") or []
    if not choices:
        _die("Cloud summary returned no choices.")
    message = choices[0].get("message") or {}
    content = message.get("content", "")
    if not isinstance(content, str) or not content.strip():
        _die("Cloud summary returned empty content.")
    return {
        "content": content.strip(),
        "request_record_count": len(payload_records),
        "response_id": data.get("id", ""),
        "model": data.get("model", model),
    }


def _clean_cloud_report(content: str) -> str:
    lines = []
    for line in content.splitlines():
        stripped = line.strip()
        if stripped.startswith("好的，") or stripped.startswith("以下是") or stripped.startswith("这是根据"):
            continue
        if re.search(r"报告生成日期|生成日期|当前日期|今天", stripped):
            continue
        lines.append(line.rstrip())
    cleaned = "\n".join(lines).strip()
    cleaned = re.sub(r"^\s*#\s*聊天记录分析报告\s*", "", cleaned).strip()
    cleaned = re.sub(r"^\s*-{3,}\s*", "", cleaned).strip()
    return cleaned


def cmd_ocr_index(args: argparse.Namespace) -> None:
    if not VISION_OCR_CLI.exists():
        _die(f"Missing OCR CLI: {VISION_OCR_CLI}")

    out_dir = Path(args.export_dir).expanduser().resolve()
    records = _records_for_ocr(out_dir, args.scope)
    cached = {} if bool(args.refresh) else _cached_ocr_records(out_dir)
    to_process: List[dict] = []
    processed: List[dict] = []
    for record in records:
        frame_path = Path(record.get("frame_path", ""))
        if frame_path.exists() and not record.get("sha256"):
            record["sha256"] = _sha256_file(frame_path)
        cache_key = (str(record.get("frame") or ""), str(record.get("sha256") or ""))
        cached_record = cached.get(cache_key)
        if cached_record:
            processed.append({**record, **cached_record, "cache_status": "reused"})
        else:
            to_process.append(record)

    max_workers = max(1, int(args.jobs or 1))
    if to_process:
        with concurrent.futures.ThreadPoolExecutor(max_workers=max_workers) as executor:
            futures = [executor.submit(_ocr_single_record, record) for record in to_process]
            for future in concurrent.futures.as_completed(futures):
                processed.append(future.result())
    processed.sort(key=lambda item: (float(item.get("timestamp_sec", 0.0)), item.get("frame", "")))
    index_path, markdown_path = _write_ocr_outputs(out_dir, processed, args.scope)
    report_path = _write_local_analysis_report(out_dir, processed)

    print(f"ocr_reused={len(processed) - len(to_process)}")
    print(f"ocr_processed={len(to_process)}")
    print(str(out_dir))
    print(str(index_path))
    print(str(markdown_path))
    print(str(report_path))

    if args.cloud == "text-summary":
        if not args.api_key:
            _die("--cloud text-summary requires --api-key")
        cloud = _call_cloud_text_summary(
            processed,
            base_url=args.base_url,
            model=args.model,
            api_key=args.api_key,
            timeout_sec=float(args.timeout),
        )
        cloud_md_path = _cloud_markdown_path(out_dir)
        cloud_content = _clean_cloud_report(cloud["content"])
        report_path.write_text(
            "# 聊天记录分析报告\n\n需人工核实。以下内容基于本地 OCR 文本和云端模型整理，不替代原始录屏、截图和律师判断。\n\n"
            + cloud_content.rstrip()
            + "\n",
            encoding="utf-8",
        )
        cloud_md_path.write_text(
            "# 云端增强聊天线索\n\n需人工核实。以下内容仅基于本地 OCR 文本整理，不替代原始录屏、截图和律师判断。\n\n"
            + cloud_content.rstrip()
            + "\n",
            encoding="utf-8",
        )
        _cloud_audit_path(out_dir).write_text(
            json.dumps(
                {
                    "status": "ok",
                    "cloud_mode": args.cloud,
                    "base_url": args.base_url,
                    "model": cloud["model"],
                    "response_id": cloud["response_id"],
                    "request_record_count": cloud["request_record_count"],
                    "sent_fields": ["frame", "timestamp_hms", "chat_time", "sha256", "chat_content"],
                    "note": "仅发送OCR文字、录屏时间、聊天时间、截图编号与sha256，未发送录屏或截图图像。",
                },
                ensure_ascii=False,
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )
        print(str(report_path))
        print(str(cloud_md_path))
        print(str(_cloud_audit_path(out_dir)))


def cmd_validate_export(args: argparse.Namespace) -> None:
    out_dir = Path(args.export_dir).expanduser().resolve()
    missing = _validate_export_dir(out_dir, require_ocr=bool(args.require_ocr))
    if missing:
        for item in missing:
            print(item, file=sys.stderr)
        _die("Export validation failed.")
    print("ok")


def _select_best_per_interval(
    raw_frames: List[Path],
    interval_sec: float,
    burst_fps: float,
    start_sec: float,
    min_sharpness: float,
    drop_blurry: bool,
    stable_motion_distance: int,
) -> List[Tuple[Path, float, float]]:
    hashes = [_dhash64(frame_path) for frame_path in raw_frames]
    candidates_by_bucket: dict[int, List[Tuple[Path, float, float, int]]] = {}
    for idx, frame_path in enumerate(raw_frames, start=1):
        ts_sec = start_sec + (idx - 1) / burst_fps
        bucket = int(((idx - 1) / burst_fps) // interval_sec)
        score = _sharpness_score(frame_path)
        hash_idx = idx - 1
        neighbor_distances: List[int] = []
        if hash_idx > 0:
            neighbor_distances.append(_hamming_distance(hashes[hash_idx], hashes[hash_idx - 1]))
        if hash_idx + 1 < len(hashes):
            neighbor_distances.append(_hamming_distance(hashes[hash_idx], hashes[hash_idx + 1]))
        motion = min(neighbor_distances) if neighbor_distances else 0
        candidates_by_bucket.setdefault(bucket, []).append((frame_path, score, ts_sec, motion))

    selected: List[Tuple[Path, float, float]] = []
    for bucket in sorted(candidates_by_bucket.keys()):
        candidates = candidates_by_bucket[bucket]
        if drop_blurry and min_sharpness > 0:
            candidates = [candidate for candidate in candidates if candidate[1] >= min_sharpness]
        if not candidates:
            continue
        stable = [candidate for candidate in candidates if candidate[3] <= stable_motion_distance]
        pool = stable or candidates
        frame_path, score, ts_sec, _motion = max(pool, key=lambda item: item[1])
        selected.append((frame_path, score, ts_sec))
    return selected


def _dedupe_selected(
    selected: List[Tuple[Path, float, float]],
    max_distance: int,
    min_visual_delta: float,
    dedupe_window: int,
    pixel_delta: float,
    preserve_head_sec: float = 0.0,
) -> Tuple[List[Tuple[Path, float, float, int]], List[dict]]:
    out: List[Tuple[Path, float, float, int]] = []
    decisions: List[dict] = []
    kept_hashes: List[int] = []
    kept_paths: List[Path] = []
    kept_thumbs: List[bytes] = []
    window = max(1, int(dedupe_window or 1))
    for frame_path, score, ts_sec in selected:
        h = _dhash64(frame_path)
        decision = {
            "source_frame": frame_path.name,
            "source_path": str(frame_path),
            "timestamp_sec": round(ts_sec, 3),
            "timestamp_hms": _format_ts(ts_sec),
            "sharpness": round(float(score), 3),
            "dhash64": f"{h:016x}",
            "decision": "keep",
            "reason": "new_content",
        }

        # 开头段落（聊天详情页等）强制保留，不做去重
        if preserve_head_sec > 0 and ts_sec <= preserve_head_sec:
            decision["decision"] = "keep"
            decision["reason"] = "head_preserved"
            kept_hashes.append(h)
            kept_paths.append(frame_path)
            kept_thumbs.append(_thumb_bytes_for_dedupe(frame_path) if pixel_delta > 0 else b"")
            decisions.append(decision)
            out.append((frame_path, score, ts_sec, h))
            continue

        recent_hashes = kept_hashes[-window:]
        recent_paths = kept_paths[-window:]
        recent_thumbs = kept_thumbs[-window:]

        best_hash_match: Tuple[int, Path] | None = None
        for prev_hash, prev_path in zip(recent_hashes, recent_paths):
            dist = _hamming_distance(prev_hash, h)
            if best_hash_match is None or dist < best_hash_match[0]:
                best_hash_match = (dist, prev_path)
        if best_hash_match is not None:
            decision["nearest_dhash_distance"] = best_hash_match[0]
            decision["nearest_dhash_frame"] = best_hash_match[1].name
        hash_duplicate = best_hash_match is not None and best_hash_match[0] <= max_distance

        thumb = b""
        if pixel_delta > 0:
            thumb = _thumb_bytes_for_dedupe(frame_path)
            best_pixel_match: Tuple[float, Path] | None = None
            for prev_thumb, prev_path in zip(recent_thumbs, recent_paths):
                diff = _mean_abs_diff_bytes(prev_thumb, thumb)
                if diff is None:
                    continue
                if best_pixel_match is None or diff < best_pixel_match[0]:
                    best_pixel_match = (diff, prev_path)
            if best_pixel_match is not None:
                decision["nearest_pixel_delta"] = round(float(best_pixel_match[0]), 3)
                decision["nearest_pixel_frame"] = best_pixel_match[1].name
                if best_pixel_match[0] <= pixel_delta:
                    decision["decision"] = "drop"
                    decision["reason"] = "cropped_pixel_duplicate"
                    decisions.append(decision)
                    continue

        if min_visual_delta > 0 and recent_paths:
            best_visual_match: Tuple[float, Path] | None = None
            for prev_path in recent_paths:
                diff = _visual_delta_score(prev_path, frame_path)
                if best_visual_match is None or diff < best_visual_match[0]:
                    best_visual_match = (diff, prev_path)
            if best_visual_match is not None:
                decision["nearest_visual_delta"] = round(float(best_visual_match[0]), 3)
                decision["nearest_visual_frame"] = best_visual_match[1].name
                if best_visual_match[0] < min_visual_delta:
                    decision["decision"] = "drop"
                    decision["reason"] = "visual_window_duplicate"
                    decisions.append(decision)
                    continue

        if hash_duplicate and pixel_delta <= 0 and min_visual_delta <= 0:
            decision["decision"] = "drop"
            decision["reason"] = "dhash_window_duplicate"
            decisions.append(decision)
            continue

        if not out:
            decision["reason"] = "first_frame"
        if not thumb and pixel_delta > 0:
            thumb = _thumb_bytes_for_dedupe(frame_path)
        if pixel_delta > 0 and thumb:
            kept_thumbs.append(thumb)
        kept_hashes.append(h)
        kept_paths.append(frame_path)
        decisions.append(decision)
        out.append((frame_path, score, ts_sec, h))
    return out, decisions


def _write_selection_log(path: Path, decisions: List[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        "\n".join(json.dumps(item, ensure_ascii=False) for item in decisions)
        + ("\n" if decisions else ""),
        encoding="utf-8",
    )


def _parse_stride_frames(value: str) -> int | str | None:
    value = (value or "").strip().lower()
    if not value:
        return None
    if value == "auto":
        return "auto"
    try:
        stride = int(value)
    except ValueError:
        _die("--stride-frames must be auto or a positive integer")
    if stride <= 0:
        _die("--stride-frames must be auto or a positive integer")
    return stride


def _parse_stride_seconds(value: str) -> float | None:
    value = (value or "").strip().lower()
    if not value:
        return None
    try:
        seconds = float(value)
    except ValueError:
        _die("--stride-seconds must be a positive number")
    if seconds <= 0:
        _die("--stride-seconds must be a positive number")
    return seconds


def _auto_stride_frames(raw_frames: List[Path]) -> int:
    if len(raw_frames) < 3:
        return 10
    hashes = [_dhash64(frame_path) for frame_path in raw_frames]
    distances = [_hamming_distance(hashes[i - 1], hashes[i]) for i in range(1, len(hashes))]
    if not distances:
        return 10
    ordered = sorted(distances)
    p75 = ordered[int(0.75 * (len(ordered) - 1))]
    if p75 >= 22:
        return 8
    if p75 >= 16:
        return 10
    if p75 >= 10:
        return 15
    return 20


def _stride_frames_from_seconds(stride_seconds: float, raw_interval_sec: float) -> int:
    if raw_interval_sec <= 0:
        _die("--raw-cache-interval must be > 0")
    return max(1, int(round(stride_seconds / raw_interval_sec)))


def _select_by_stride_frames(
    raw_frames: List[Path],
    burst_fps: float,
    start_sec: float,
    stride_frames: int | str,
    preserve_head_sec: float,
) -> Tuple[List[Tuple[Path, float, float, int]], List[dict], int]:
    stride = _auto_stride_frames(raw_frames) if stride_frames == "auto" else int(stride_frames)
    stride = max(1, stride)
    head_stride = max(1, int(round(burst_fps / 3.0)))
    selected: List[Tuple[Path, float, float, int]] = []
    decisions: List[dict] = []
    post_head_seen = 0

    for idx, frame_path in enumerate(raw_frames, start=1):
        ts_sec = start_sec + (idx - 1) / burst_fps
        in_head = preserve_head_sec > 0 and ts_sec <= preserve_head_sec
        keep = False
        reason = "stride_skip"
        if in_head:
            keep = ((idx - 1) % head_stride) == 0
            reason = "head_preserved_stride" if keep else "head_stride_skip"
        else:
            keep = (post_head_seen % stride) == 0
            reason = f"stride_{stride}" if keep else "stride_skip"
            post_head_seen += 1

        h = _dhash64(frame_path) if keep else None
        decision = {
            "source_frame": frame_path.name,
            "source_path": str(frame_path),
            "timestamp_sec": round(ts_sec, 3),
            "timestamp_hms": _format_ts(ts_sec),
            "decision": "keep" if keep else "drop",
            "reason": reason,
            "stride_frames": stride,
            "head_stride_frames": head_stride,
        }
        if h is not None:
            score = _sharpness_score(frame_path)
            decision["sharpness"] = round(float(score), 3)
            decision["dhash64"] = f"{h:016x}"
            selected.append((frame_path, score, ts_sec, h))
        decisions.append(decision)
    return selected, decisions, stride


def _materialize_selected_frames(
    selected_frames: List[Tuple[Path, float, float, int]],
    frames_dir: Path,
) -> Tuple[List[Path], List[dict]]:
    frames_dir.mkdir(parents=True, exist_ok=True)
    materialized: List[Path] = []
    records: List[dict] = []
    for out_idx, (source_path, sharpness, ts_sec, dhash) in enumerate(selected_frames, start=1):
        dst = frames_dir / f"frame_{out_idx:06d}{source_path.suffix}"
        _link_or_copy(source_path, dst)
        materialized.append(dst)
        records.append(
            {
                "frame": dst.name,
                "frame_path": str(dst),
                "source_frame": source_path.name,
                "source_path": str(source_path),
                "timestamp_sec": round(ts_sec, 3),
                "timestamp_hms": _format_ts(ts_sec),
                "sharpness": round(float(sharpness), 3),
                "dhash64": f"{dhash:016x}",
                "sha256": _sha256_file(source_path),
            }
        )
    return materialized, records


def cmd_interval_pdf(args: argparse.Namespace) -> None:
    _ensure_bins(["ffmpeg"])

    videos = [Path(v).expanduser().resolve() for v in args.video]
    for v in videos:
        if not v.exists():
            _die(f"Video not found: {v}")

    variants: List[float] = list(args.variants or [])
    stride_frames = _parse_stride_frames(getattr(args, "stride_frames", ""))
    stride_seconds = _parse_stride_seconds(getattr(args, "stride_seconds", ""))
    raw_cache_interval = float(getattr(args, "raw_cache_interval", 0.0) or 0.0)
    raw_cache_dir_arg = (getattr(args, "raw_cache_dir", "") or "").strip()
    reuse_raw_only = bool(getattr(args, "reuse_raw_only", False))
    if stride_frames is not None and stride_seconds is not None:
        _die("--stride-frames and --stride-seconds cannot be used together.")
    if variants:
        if any(v <= 0 for v in variants):
            _die("--variants values must be > 0")
        if len(set(variants)) != len(variants):
            _die("--variants contains duplicates")
        if args.filter != "auto":
            _die("--variants requires --filter auto (so it can reuse 原始抽帧 for all variants).")
        if args.pdf:
            _die("--pdf cannot be used with --variants (multiple outputs).")
        if stride_frames is not None:
            _die("--stride-frames cannot be used with --variants.")
        if stride_seconds is not None:
            _die("--stride-seconds cannot be used with --variants.")

    if args.pdf and len(videos) != 1:
        _die("--pdf can only be used with a single input video (otherwise outputs would overwrite).")

    if args.interval <= 0:
        _die("--interval must be > 0")
    if args.start < 0:
        _die("--start must be >= 0")
    if args.duration is not None and args.duration < 0:
        _die("--duration must be >= 0")
    if args.max_frames is not None and args.max_frames < 0:
        _die("--max-frames must be >= 0")
    if args.max_width is not None and args.max_width < 0:
        _die("--max-width must be >= 0")
    if args.filter not in {"off", "auto"}:
        _die("--filter must be one of: off, auto")
    if args.burst_fps is not None and args.burst_fps < 0:
        _die("--burst-fps must be >= 0")
    if raw_cache_interval is not None and raw_cache_interval < 0:
        _die("--raw-cache-interval must be >= 0")
    if args.max_raw_frames is not None and args.max_raw_frames < 0:
        _die("--max-raw-frames must be >= 0")
    if args.dedupe_distance is not None and args.dedupe_distance < 0:
        _die("--dedupe-distance must be >= 0")
    if args.dedupe_window is not None and args.dedupe_window < 1:
        _die("--dedupe-window must be >= 1")
    if args.min_visual_delta is not None and args.min_visual_delta < 0:
        _die("--min-visual-delta must be >= 0")
    if args.pixel_delta is not None and args.pixel_delta < 0:
        _die("--pixel-delta must be >= 0")
    if args.stable_motion_distance is not None and args.stable_motion_distance < 0:
        _die("--stable-motion-distance must be >= 0")
    if args.min_sharpness is not None and args.min_sharpness < 0:
        _die("--min-sharpness must be >= 0")

    out_base = Path(args.out_base).expanduser().resolve()

    if args.out_dir:
        if len(videos) != 1:
            _die("--out-dir can only be used with a single input video.")
        out_dirs = [Path(args.out_dir).expanduser().resolve()]
    else:
        out_dirs = [_unique_out_dir(out_base, v) for v in videos]

    keep_frames = args.keep_frames

    for video_path, out_dir in zip(videos, out_dirs):
        out_dir.mkdir(parents=True, exist_ok=True)
        frames_dir = _final_frames_dir(out_dir)
        index_path = _frame_index_path(out_dir)
        pdf_path = Path(args.pdf).expanduser().resolve() if args.pdf else _pdf_path(out_dir)

        warn_interval = min(variants) if variants else args.interval
        _warn_if_huge(warn_interval, args.start, args.duration, args.max_frames or 0)

        if args.filter == "off":
            if variants:
                _die("--variants cannot be used with --filter off.")
            frames = _extract_frames(
                video_path=video_path,
                frames_dir=frames_dir,
                interval_sec=args.interval,
                image_ext=args.image_ext,
                start_sec=args.start,
                duration_sec=args.duration,
                max_width=args.max_width or 0,
            )

            if args.max_frames and args.max_frames > 0:
                frames = frames[: args.max_frames]

            _write_index_jsonl(frames, index_path, interval_sec=args.interval, start_sec=args.start, include_sha256=True)
            _build_pdf_streaming(frames, pdf_path, jpeg_quality=args.pdf_jpeg_quality)

            if not keep_frames:
                try:
                    shutil.rmtree(frames_dir)
                except Exception as e:
                    print(f"Warning: failed to delete frames dir: {frames_dir}: {e}", file=sys.stderr)
        else:
            if raw_cache_interval > 0:
                burst_fps = 1.0 / raw_cache_interval
            else:
                burst_fps = float(args.burst_fps) if args.burst_fps and args.burst_fps > 0 else 8.0
                raw_cache_interval = 1.0 / burst_fps
            review_dir = _review_dir(out_dir)
            review_dir.mkdir(parents=True, exist_ok=True)
            raw_dir = Path(raw_cache_dir_arg).expanduser().resolve() if raw_cache_dir_arg else _raw_frames_dir(out_dir)
            raw_index_path = review_dir / "原始抽帧索引.jsonl"
            selected_index_path = _selected_index_path(out_dir)

            raw_frames = _list_raw_frames(raw_dir, args.image_ext)
            if raw_frames:
                print(f"reuse_raw_cache={raw_dir}", file=sys.stderr)
            else:
                if reuse_raw_only:
                    _die(f"Raw frame cache not found or empty: {raw_dir}")
                raw_frames = _extract_frames_burst(
                    video_path=video_path,
                    frames_dir=raw_dir,
                    burst_fps=burst_fps,
                    image_ext=args.image_ext,
                    start_sec=args.start,
                    duration_sec=args.duration,
                    max_width=args.max_width or 0,
                    max_raw_frames=args.max_raw_frames,
                )
                print(f"created_raw_cache={raw_dir}", file=sys.stderr)
            _write_index_jsonl(
                raw_frames,
                raw_index_path,
                interval_sec=raw_cache_interval,
                start_sec=args.start,
                include_sha256=False,
            )

            intervals = variants if variants else [float(args.interval)]
            dedupe_distance = int(args.dedupe_distance) if args.dedupe_distance is not None else 4
            dedupe_window = int(args.dedupe_window) if args.dedupe_window is not None else 6
            min_visual_delta = float(args.min_visual_delta) if args.min_visual_delta is not None else 1.5
            pixel_delta = float(args.pixel_delta) if args.pixel_delta is not None else 0.02
            stable_motion_distance = (
                int(args.stable_motion_distance) if args.stable_motion_distance is not None else 18
            )

            def _export_variant(
                variant_interval: float,
                variant_root: Path,
                label: str,
            ) -> Tuple[str, Path, Path]:
                variant_root.mkdir(parents=True, exist_ok=True)
                v_frames_dir = _final_frames_dir(variant_root, label)
                v_index_path = _frame_index_path(variant_root, label)
                v_pdf_path = _pdf_path(variant_root, label)
                v_selected_index_path = _selected_index_path(variant_root, label)

                selected = _select_best_per_interval(
                    raw_frames=raw_frames,
                    interval_sec=variant_interval,
                    burst_fps=burst_fps,
                    start_sec=args.start,
                    min_sharpness=float(args.min_sharpness or 0.0),
                    drop_blurry=bool(args.drop_blurry),
                    stable_motion_distance=stable_motion_distance,
                )
                selected_deduped, selection_decisions = _dedupe_selected(
                    selected,
                    max_distance=dedupe_distance,
                    min_visual_delta=min_visual_delta,
                    dedupe_window=dedupe_window,
                    pixel_delta=pixel_delta,
                    preserve_head_sec=float(args.preserve_head_sec or 0.0),
                )
                if args.max_frames and args.max_frames > 0:
                    selected_deduped = selected_deduped[: args.max_frames]

                materialized, selected_records = _materialize_selected_frames(selected_deduped, frames_dir=v_frames_dir)

                _write_selection_log(v_selected_index_path, selection_decisions)

                v_index_path.write_text(
                    "\n".join(
                        json.dumps(
                            {
                                "frame": r["frame"],
                                "frame_path": r["frame_path"],
                                "timestamp_sec": r["timestamp_sec"],
                                "timestamp_hms": r["timestamp_hms"],
                                "sha256": r["sha256"],
                            },
                            ensure_ascii=False,
                        )
                        for r in selected_records
                    )
                    + ("\n" if selected_records else ""),
                    encoding="utf-8",
                )

                _build_pdf_streaming(materialized, v_pdf_path, jpeg_quality=args.pdf_jpeg_quality)

                if not keep_frames:
                    try:
                        shutil.rmtree(v_frames_dir)
                    except Exception as e:
                        print(f"Warning: failed to delete frames dir: {v_frames_dir}: {e}", file=sys.stderr)

                return label, v_pdf_path, v_index_path

            if variants:
                exported_v: List[Tuple[str, Path, Path]] = []
                for variant_interval in intervals:
                    label = _variant_label(variant_interval)
                    exported_v.append(_export_variant(variant_interval, out_dir, label))
            else:
                if stride_seconds is not None:
                    actual_stride = _stride_frames_from_seconds(stride_seconds, raw_cache_interval)
                    selected_deduped, selection_decisions, actual_stride = _select_by_stride_frames(
                        raw_frames=raw_frames,
                        burst_fps=burst_fps,
                        start_sec=args.start,
                        stride_frames=actual_stride,
                        preserve_head_sec=float(args.preserve_head_sec or 0.0),
                    )
                    print(f"stride_seconds={stride_seconds}", file=sys.stderr)
                    print(f"raw_cache_interval={raw_cache_interval}", file=sys.stderr)
                    print(f"stride_frames={actual_stride}", file=sys.stderr)
                elif stride_frames is not None:
                    selected_deduped, selection_decisions, actual_stride = _select_by_stride_frames(
                        raw_frames=raw_frames,
                        burst_fps=burst_fps,
                        start_sec=args.start,
                        stride_frames=stride_frames,
                        preserve_head_sec=float(args.preserve_head_sec or 0.0),
                    )
                    print(f"stride_frames={actual_stride}", file=sys.stderr)
                else:
                    selected = _select_best_per_interval(
                        raw_frames=raw_frames,
                        interval_sec=float(args.interval),
                        burst_fps=burst_fps,
                        start_sec=args.start,
                        min_sharpness=float(args.min_sharpness or 0.0),
                        drop_blurry=bool(args.drop_blurry),
                        stable_motion_distance=stable_motion_distance,
                    )

                    selected_deduped, selection_decisions = _dedupe_selected(
                        selected,
                        max_distance=dedupe_distance,
                        min_visual_delta=min_visual_delta,
                        dedupe_window=dedupe_window,
                        pixel_delta=pixel_delta,
                        preserve_head_sec=float(args.preserve_head_sec or 0.0),
                    )

                if args.max_frames and args.max_frames > 0:
                    selected_deduped = selected_deduped[: args.max_frames]

                materialized, selected_records = _materialize_selected_frames(selected_deduped, frames_dir=frames_dir)

                _write_selection_log(selected_index_path, selection_decisions)

                index_path.write_text(
                    "\n".join(
                        json.dumps(
                            {
                                "frame": r["frame"],
                                "frame_path": r["frame_path"],
                                "timestamp_sec": r["timestamp_sec"],
                                "timestamp_hms": r["timestamp_hms"],
                                "sha256": r["sha256"],
                            },
                            ensure_ascii=False,
                        )
                        for r in selected_records
                    )
                    + ("\n" if selected_records else ""),
                    encoding="utf-8",
                )

                _build_pdf_streaming(materialized, pdf_path, jpeg_quality=args.pdf_jpeg_quality)

                if not keep_frames:
                    try:
                        shutil.rmtree(frames_dir)
                    except Exception as e:
                        print(f"Warning: failed to delete frames dir: {frames_dir}: {e}", file=sys.stderr)

            if not args.keep_raw:
                try:
                    shutil.rmtree(raw_dir)
                except Exception as e:
                    print(f"Warning: failed to delete raw frames dir: {raw_dir}: {e}", file=sys.stderr)

        if variants:
            print(str(out_dir))
            for label, v_pdf, v_index in exported_v:
                print(label)
                print(str(v_pdf))
                print(str(v_index))
        else:
            print(str(out_dir))
            print(str(pdf_path))
            print(str(index_path))


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="WeChat chat screen-recording evidence exporter (interval screenshots -> PDF)"
    )
    sub = parser.add_subparsers(dest="command", required=True)

    p = sub.add_parser("interval-pdf", help="Extract fixed-interval screenshots and build PDF")
    p.add_argument("video", nargs="+", help="Input video path(s)")
    p.add_argument("--interval", type=float, default=1.0, help="Screenshot interval in seconds (default: 1.0)")
    p.add_argument("--preserve-head-sec", type=float, default=8.0,
                    help="Seconds from the start to preserve without deduplication (default: 8.0). "
                         "WeChat chat detail pages at the beginning are always kept.")
    p.add_argument("--image-ext", choices=["png", "jpg"], default="png", help="Screenshot image format (default: png)")
    p.add_argument("--out-base", default=str(DEFAULT_OUT_BASE), help=f"Base output dir (default: {DEFAULT_OUT_BASE})")
    p.add_argument("--out-dir", default="", help="Explicit output directory (single video only)")
    p.add_argument("--pdf", default="", help="Explicit PDF output path (default: <out-dir>/录屏取证初稿.pdf)")
    p.add_argument("--start", type=float, default=0.0, help="Start offset in seconds")
    p.add_argument("--duration", type=float, default=None, help="Optional duration limit in seconds")
    p.add_argument("--max-frames", type=int, default=None, help="Cap maximum number of frames (optional)")
    p.add_argument("--max-width", type=int, default=None, help="Max image width (shrink only; optional)")
    p.add_argument("--keep-frames", action="store_true", default=True, help="Keep extracted frames (default)")
    p.add_argument("--no-keep-frames", dest="keep_frames", action="store_false", help="Delete frames after PDF export")
    p.add_argument("--filter", choices=["off", "auto"], default="off", help="Auto-pick clearer & less-duplicate frames (default: off)")
    p.add_argument("--burst-fps", type=float, default=None, help="When --filter auto: raw sampling FPS (default: 8)")
    p.add_argument("--max-raw-frames", type=int, default=None, help="When --filter auto: cap raw extracted frames (optional)")
    p.add_argument("--dedupe-distance", type=int, default=None, help="When --filter auto: dHash distance threshold (default: 4)")
    p.add_argument("--dedupe-window", type=int, default=None, help="When --filter auto: compare against this many recent kept frames (default: 6)")
    p.add_argument("--min-visual-delta", type=float, default=None, help="When --filter auto: skip adjacent outputs with very low visual difference (default: 1.5)")
    p.add_argument("--pixel-delta", type=float, default=None, help="When --filter auto: cropped thumbnail mean-difference threshold (default: 0.02)")
    p.add_argument("--stable-motion-distance", type=int, default=None, help="When --filter auto: prefer frames whose neighbor dHash distance is at most this value (default: 18)")
    p.add_argument("--stride-frames", default="", help="When --filter auto: keep every N raw frames, or auto to infer N from scroll speed")
    p.add_argument("--stride-seconds", default="", help="When --filter auto: keep one frame every N seconds from raw cache")
    p.add_argument("--raw-cache-dir", default="", help="When --filter auto: shared raw frame cache dir; reuse if it already has raw frames")
    p.add_argument("--raw-cache-interval", type=float, default=0.0, help="Seconds between frames in the shared raw cache")
    p.add_argument("--reuse-raw-only", action="store_true", help="Reuse existing raw frame cache only; fail instead of extracting video")
    p.add_argument("--min-sharpness", type=float, default=None, help="When --filter auto: minimum sharpness score (optional)")
    p.add_argument("--drop-blurry", action="store_true", help="When --filter auto: drop buckets below --min-sharpness")
    p.add_argument("--keep-raw", action="store_true", default=True, help="When --filter auto: keep 原始抽帧/ (default)")
    p.add_argument("--no-keep-raw", dest="keep_raw", action="store_false", help="When --filter auto: delete 原始抽帧/ after export")
    p.add_argument("--variants", nargs="+", type=float, default=[], help="When --filter auto: output multiple interval variants (example: --variants 2 3)")
    p.add_argument("--pdf-jpeg-quality", type=int, default=88, help="JPEG quality used inside PDF to control file size (default: 88)")
    p.set_defaults(func=cmd_interval_pdf)

    p = sub.add_parser("ocr-index", help="OCR selected or raw frames into reviewable index files")
    p.add_argument("export_dir", help="Export directory produced by interval-pdf")
    p.add_argument("--scope", choices=["selected", "raw"], default="selected", help="OCR selected screenshots or raw extracted frames")
    p.add_argument("--jobs", type=int, default=2, help="Parallel OCR jobs (default: 2)")
    p.add_argument("--refresh", action="store_true", help="Ignore existing OCR index and re-OCR every frame")
    p.add_argument("--cloud", choices=["off", "text-summary"], default="off", help="Optional cloud text-only enhancement")
    p.add_argument("--base-url", default="https://api.deepseek.com", help="OpenAI-compatible base URL")
    p.add_argument("--model", default="deepseek-chat", help="OpenAI-compatible model name")
    p.add_argument("--api-key", default="", help="API key for cloud text enhancement")
    p.add_argument("--timeout", type=float, default=120.0, help="Cloud request timeout in seconds")
    p.set_defaults(func=cmd_ocr_index)

    p = sub.add_parser("validate-export", help="Validate export directory structure")
    p.add_argument("export_dir", help="Export directory produced by interval-pdf")
    p.add_argument("--require-ocr", action="store_true", help="Require OCR index outputs")
    p.set_defaults(func=cmd_validate_export)

    return parser


def main() -> None:
    parser = build_parser()
    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
