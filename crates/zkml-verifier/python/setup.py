"""Setup script for zkml-verifier Python package.

During `pip install`, this builds the Rust cdylib and copies the shared library
into the zkml_verifier package directory so it ships inside the wheel.
"""

import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

from setuptools import setup
from setuptools.command.build_py import build_py


# Path to the Rust crate root (two levels up from python/)
CRATE_ROOT = Path(__file__).parent.parent.resolve()


def _lib_filename():
    """Return the platform-specific shared library filename."""
    system = platform.system()
    if system == "Darwin":
        return "libzkml_verifier.dylib"
    elif system == "Linux":
        return "libzkml_verifier.so"
    elif system == "Windows":
        return "zkml_verifier.dll"
    else:
        raise OSError(f"Unsupported platform: {system}")


def _cargo_build():
    """Run cargo build --release for the zkml-verifier cdylib."""
    target_dir = os.environ.get("CARGO_TARGET_DIR", str(CRATE_ROOT / "target"))
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = target_dir

    cmd = [
        "cargo", "build",
        "--release",
        "--lib",
        "--manifest-path", str(CRATE_ROOT / "Cargo.toml"),
    ]

    print(f"Building Rust library: {' '.join(cmd)}")
    result = subprocess.run(cmd, env=env, capture_output=True, text=True)
    if result.returncode != 0:
        print(result.stdout, file=sys.stdout)
        print(result.stderr, file=sys.stderr)
        raise RuntimeError(
            f"cargo build failed with exit code {result.returncode}.\n"
            f"Make sure Rust is installed: https://rustup.rs/\n"
            f"stderr: {result.stderr[-500:]}"
        )

    lib_path = Path(target_dir) / "release" / _lib_filename()
    if not lib_path.exists():
        raise FileNotFoundError(
            f"Expected library at {lib_path} but it was not found. "
            f"Check cargo build output above."
        )
    return lib_path


class BuildWithCargo(build_py):
    """Custom build_py that compiles the Rust library and bundles it."""

    def run(self):
        # If ZKML_SKIP_BUILD is set (e.g., in CI where lib is pre-built),
        # look for the library in the expected locations instead of building.
        if os.environ.get("ZKML_SKIP_BUILD"):
            lib_path = self._find_prebuilt()
        else:
            lib_path = _cargo_build()

        # Copy the compiled library into the package directory
        pkg_dir = Path(self.build_lib) / "zkml_verifier"
        pkg_dir.mkdir(parents=True, exist_ok=True)
        dest = pkg_dir / _lib_filename()
        print(f"Copying {lib_path} -> {dest}")
        shutil.copy2(str(lib_path), str(dest))

        # Also copy into the source tree for editable installs
        src_pkg_dir = Path(__file__).parent / "zkml_verifier"
        src_dest = src_pkg_dir / _lib_filename()
        if not src_dest.exists():
            print(f"Copying {lib_path} -> {src_dest} (editable install)")
            shutil.copy2(str(lib_path), str(src_dest))

        super().run()

    def _find_prebuilt(self):
        """Find a pre-built library (for CI pipelines that build separately)."""
        lib_name = _lib_filename()
        search_dirs = [
            os.environ.get("ZKML_LIB_DIR", ""),
            str(CRATE_ROOT / "target" / "release"),
            str(Path(__file__).parent.parent.parent.parent / "target" / "release"),
        ]
        for d in search_dirs:
            if not d:
                continue
            p = Path(d) / lib_name
            if p.exists():
                return p
        raise FileNotFoundError(
            f"ZKML_SKIP_BUILD is set but could not find {lib_name}. "
            f"Build with: cargo build --release -p zkml-verifier"
        )


setup(
    cmdclass={"build_py": BuildWithCargo},
    # Package data: include the shared library in the wheel
    package_data={"zkml_verifier": ["*.so", "*.dylib", "*.dll"]},
)
