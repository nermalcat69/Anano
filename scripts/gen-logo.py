#!/usr/bin/env python3
"""把 assets/logo/icon-source.png（1024×1024 的完整方形图标，无需再合成）缩放成
应用需要的三种尺寸：一份小 PNG、一份 macOS 用的 1024 PNG、一份 Windows 用的 ICO。

  scripts/gen-logo.py

产物：
  crates/anano-app/assets/logo/anano.png         512×512 —— app 侧栏 / Linux 窗口图标用
  crates/anano-app/resources/macos/anano-1024.png
                                                  1024×1024 —— macOS .icns 用
  crates/anano-app/resources/windows/anano.ico   多尺寸（16…256）
                                                  —— build.rs 嵌进 exe 资源（资源管理器 / 任务栏 / 窗口图标）

源图是设计师直接导出的「App Store 1024×1024」格式：本身已经是成品方图（不透明、
带自己的投影），不需要再叠加圆角遮罩 / 渐变底板 / 阴影——那一整套合成逻辑是给旧版
只有一只裁切猫的透明 PNG 用的，现在的源图已经不是那种素材，直接按目标尺寸重采样
即可（macOS 11+ 的 Dock / Finder 会自动把方图套进 squircle，不需要我们手工预裁）。

只在 logo 改动时由开发者手动跑一次并提交产物；CI 不调用本脚本。依赖：
pip install pillow
"""

from pathlib import Path

from PIL import Image

REPO_ROOT = Path(__file__).resolve().parent.parent
SOURCE = REPO_ROOT / "crates/anano-app/assets/logo/icon-source.png"
APP_LOGO = REPO_ROOT / "crates/anano-app/assets/logo/anano.png"
MACOS_ICON = REPO_ROOT / "crates/anano-app/resources/macos/anano-1024.png"
WINDOWS_ICON = REPO_ROOT / "crates/anano-app/resources/windows/anano.ico"

APP_LOGO_SIZE = 512
MACOS_CANVAS = 1024
# Windows 图标习惯提供的档位；256 那档 Pillow 会自动用 PNG 压缩存放
ICO_SIZES = (16, 24, 32, 48, 64, 128, 256)


def resized(source: Image.Image, side: int) -> Image.Image:
    return source.resize((side, side), Image.Resampling.LANCZOS)


def main() -> None:
    source = Image.open(SOURCE).convert("RGBA")

    APP_LOGO.parent.mkdir(parents=True, exist_ok=True)
    app_logo = resized(source, APP_LOGO_SIZE)
    app_logo.save(APP_LOGO, optimize=True)
    print(f"已写入 {APP_LOGO.relative_to(REPO_ROOT)}（{app_logo.width}×{app_logo.height}）")

    MACOS_ICON.parent.mkdir(parents=True, exist_ok=True)
    macos_icon = resized(source, MACOS_CANVAS) if source.size != (MACOS_CANVAS, MACOS_CANVAS) else source
    macos_icon.save(MACOS_ICON, optimize=True)
    print(f"已写入 {MACOS_ICON.relative_to(REPO_ROOT)}（{macos_icon.width}×{macos_icon.height}）")

    WINDOWS_ICON.parent.mkdir(parents=True, exist_ok=True)
    ico_source = resized(source, max(ICO_SIZES))
    ico_source.save(WINDOWS_ICON, format="ICO", sizes=[(s, s) for s in ICO_SIZES])
    print(f"已写入 {WINDOWS_ICON.relative_to(REPO_ROOT)}（{', '.join(map(str, ICO_SIZES))}）")


if __name__ == "__main__":
    main()
