#!/usr/bin/env python3
"""Install the built Linux app, desktop entry and SVG/PNG icons for this user."""

import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


def desktop_exec(path: Path) -> str:
    # Exec quoting is followed by the desktop file's general string escaping.
    value = str(path).replace("%", "%%")
    for char in ('\\', '"', '`', '$'):
        value = value.replace(char, "\\" + char)
    return ('"' + value + '"').replace("\\", "\\\\")


def install(source: Path, destination: Path, mode: int = 0o644) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    # Replacing a staged file also works while an older executable is running.
    descriptor, temporary = tempfile.mkstemp(prefix=".heya-", dir=destination.parent)
    try:
        with os.fdopen(descriptor, "wb") as output, source.open("rb") as input_file:
            shutil.copyfileobj(input_file, output)
            os.fchmod(output.fileno(), mode)
        os.replace(temporary, destination)
    finally:
        Path(temporary).unlink(missing_ok=True)


def main() -> None:
    if sys.platform != "linux":
        sys.exit("This desktop installer is for Linux.")
    if len(sys.argv) != 1:
        sys.exit("Usage: python3 scripts/install-desktop.py")
    root = Path(__file__).resolve().parent.parent
    binary = root / "target/release/heya"
    if not binary.is_file() or not os.access(binary, os.X_OK):
        sys.exit("Build first: cargo build --release --locked")
    for command in ("rsvg-convert", "desktop-file-validate"):
        if shutil.which(command) is None:
            sys.exit(
                f"Missing {command}. Install with: "
                "sudo pacman -S librsvg desktop-file-utils"
            )

    data_home = Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local/share")
    if not data_home.is_absolute():
        sys.exit("XDG_DATA_HOME must be an absolute path.")
    installed_binary = Path.home() / ".local/bin/heya"
    application_id = "org.heya.Heya"
    icon_root = data_home / "icons/hicolor"
    applications = data_home / "applications"
    source_icon = root / "ui/assets/heya.svg"

    with tempfile.TemporaryDirectory(prefix="heya-desktop-") as temporary:
        stage = Path(temporary)
        desktop = stage / f"{application_id}.desktop"
        template = (root / "packaging/linux" / desktop.name).read_text()
        desktop.write_text(
            template.replace("Exec=heya\n", f"Exec={desktop_exec(installed_binary)}\n")
        )
        subprocess.run(["desktop-file-validate", str(desktop)], check=True)
        sizes = (16, 24, 32, 48, 64, 128, 256, 512)
        for size in sizes:
            subprocess.run(
                [
                    "rsvg-convert", "--width", str(size), "--height", str(size),
                    "--output", str(stage / f"{size}.png"), str(source_icon),
                ],
                check=True,
            )

        # Finish rendering and validation before changing the user's installation.
        install(binary, installed_binary, 0o755)
        install(source_icon, icon_root / "scalable/apps" / f"{application_id}.svg")
        for size in sizes:
            install(
                stage / f"{size}.png",
                icon_root / f"{size}x{size}/apps" / f"{application_id}.png",
            )
        install(desktop, applications / desktop.name)

    for command in (
        ["update-desktop-database", str(applications)],
        ["gtk-update-icon-cache", "--force", "--ignore-theme-index", str(icon_root)],
    ):
        if shutil.which(command[0]):
            result = subprocess.run(command, check=False)
            if result.returncode:
                print(f"Desktop cache refresh failed: {command[0]}", file=sys.stderr)
    print(f"Installed Heya: {installed_binary}")
    print(f"Desktop entry: {applications / desktop.name}")
    print(f"SVG icon: {icon_root / 'scalable/apps' / (application_id + '.svg')}")
    print("Launch Heya from the application menu. Rerun this installer after rebuilding.")


if __name__ == "__main__":
    main()
