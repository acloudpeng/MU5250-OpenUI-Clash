#!/usr/bin/env python3
"""Build lpac (the eUICC LPA used by EasyEUICC) as a static linux/arm64 binary.

Pinned to the v2.3.0 release: later upstream versions load their drivers with dlopen(),
which a static binary cannot do. Only the stdio APDU and HTTP drivers are built; the
agent answers them, sending APDUs to the modem over QMI and SM-DP+ requests over HTTPS.
Applies scripts/lpac-patches/*.patch, each a backport of an upstream lpac fix (see each
header). Compiles with Zig, as the screen build does, without CMake.
Does not contact or change the modem.

lpac's src/ and driver/ are AGPL-3.0-only and euicc/ is LGPL-2.1-only; the binary is a
separate program the agent runs. The source archive is SHA-256 pinned; see docs/ESIM.md.
"""
import argparse
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request

COMMIT = 'c2fcf5e4b21c712d54e35a11da2ad9ad134fb821'  # tag v2.3.0
ARCHIVE_URL = f'https://codeload.github.com/estkme-group/lpac/tar.gz/{COMMIT}'
ARCHIVE_SHA256 = 'f3e92f08d10121cf3633cce66853ba1260eb5daa35c594f903cc2720fc11f9ee'
PATCHES = sorted((Path(__file__).resolve().parent / 'lpac-patches').glob('*.patch'))
# Bump the suffix whenever the patches change.
VERSION = 'v2.3.0-openui1'
ZIG_VERSION = '0.14.1'
TARGETS = {'aarch64': 'aarch64-linux-musl', 'x86_64': 'x86_64-linux-musl'}
# Reproducible with the pinned archive, patches and Zig version; any change must update this.
SHA256 = '66641fd6ec38a96ea4a23dc28f22c4de9b75e4252a06e35ae8a233efb4792c98'
ROOT = Path(__file__).resolve().parents[1]
CACHE = ROOT / 'build' / 'lpac-deps'
SOURCES = [
    'cjson/cJSON.c', 'cjson/cJSON_ex.c',
    'utils/lpac/utils.c',
    'driver/driver.c', 'driver/apdu/stdio.c', 'driver/http/stdio.c',
]
SOURCE_DIRS = ['euicc', 'src', 'src/applet', 'src/applet/chip', 'src/applet/notification',
               'src/applet/profile']
FLAGS = ['-std=gnu99', '-O2', '-ffunction-sections', '-fdata-sections', '-fno-ident']


def run(args, **kwargs):
    subprocess.run(args, check=True, **kwargs)


def fetch():
    CACHE.mkdir(parents=True, exist_ok=True)
    archive = CACHE / f'lpac-{COMMIT[:8]}.tar.gz'
    if not archive.exists():
        with urllib.request.urlopen(ARCHIVE_URL, timeout=60) as response:
            data = response.read(16 * 1024 * 1024)
        archive.write_bytes(data)
    if hashlib.sha256(archive.read_bytes()).hexdigest() != ARCHIVE_SHA256:
        archive.unlink()
        raise SystemExit('lpac source archive checksum mismatch')
    return archive


def extract(archive, into):
    with tarfile.open(archive) as tar:
        for member in tar.getmembers():
            if not (into / member.name).resolve().is_relative_to(into.resolve()) \
                    or member.issym() or member.islnk() or member.isdev():
                raise SystemExit(f'Unsafe archive member: {member.name}')
        # Members are checked above; the 'data' filter also drops odd permissions.
        tar.extractall(into, **({'filter': 'data'} if hasattr(tarfile, 'data_filter') else {}))
    (source,) = into.iterdir()
    for patch in PATCHES:
        run(['git', 'apply', '--whitespace=nowarn', str(patch)], cwd=source)
    return source


def zig():
    path = os.environ.get('ZIG') or shutil.which('zig')
    if not path:
        raise SystemExit(f'Need Zig {ZIG_VERSION}: set ZIG=/path/to/zig')
    version = subprocess.run([path, 'version'], capture_output=True, text=True).stdout.strip()
    if version != ZIG_VERSION:
        raise SystemExit(f'Need Zig {ZIG_VERSION}, found {version or "none"} at {path}')
    return path


def units(source):
    files = [source / f for f in SOURCES]
    for d in SOURCE_DIRS:
        files += sorted((source / d).glob('*.c'))
    return files


def build(source, target, output):
    cc = [zig(), 'cc', '-target', TARGETS[target]]
    includes = [f'-I{source / d}' for d in ('.', 'src', 'utils', 'driver')]
    # src/applet/version.c falls back to "v0.0.0-unknown" unless this is defined.
    defines = [f'-DLPAC_VERSION="{VERSION}"']
    # Keep the temporary build path out of the binary so rebuilds are byte-identical.
    remap = [f'-ffile-prefix-map={source}=lpac']
    with tempfile.TemporaryDirectory(prefix='openui-lpac-obj-') as tmp:
        objects = []
        for i, unit in enumerate(units(source)):
            obj = Path(tmp) / f'{i:03d}-{unit.stem}.o'
            run(cc + FLAGS + remap + defines + includes + ['-c', str(unit), '-o', str(obj)])
            objects.append(str(obj))
        run(cc + objects + ['-static', '-s', '-Wl,--gc-sections', '-Wl,--build-id=none',
                            '-o', str(output)])


def check_elf(path, target):
    header = path.read_bytes()[:64]
    machine = {'aarch64': 183, 'x86_64': 62}[target]
    if header[:5] != b'\x7fELF\x02' or header[18:20] != machine.to_bytes(2, 'little'):
        raise SystemExit(f'lpac output is not a {target} ELF')
    if b'/lib/ld-' in path.read_bytes()[:4096]:
        raise SystemExit('lpac output is dynamically linked')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', choices=sorted(TARGETS), default='aarch64',
                        help='x86_64 is for the simulated router used in end-to-end tests')
    parser.add_argument('--output', help='default: build/lpac/lpac (aarch64) or build/lpac/lpac-x86_64')
    parser.add_argument('--no-verify', action='store_true', help='accept a checksum other than SHA256')
    args = parser.parse_args()
    default = 'lpac' if args.target == 'aarch64' else f'lpac-{args.target}'
    output = Path(args.output or ROOT / 'build' / 'lpac' / default).resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    archive = fetch()
    with tempfile.TemporaryDirectory(prefix='openui-lpac-') as tmp:
        source = extract(archive, Path(tmp))
        build(source, args.target, output)
    check_elf(output, args.target)
    output.chmod(0o755)
    digest = hashlib.sha256(output.read_bytes()).hexdigest()
    if args.target == 'aarch64' and digest != SHA256 and not args.no_verify:
        raise SystemExit(f'lpac checksum {digest} does not match the pinned {SHA256}')
    output.with_name(output.name + '.sha256').write_text(f'{digest}  {output.name}\n')
    size = output.stat().st_size / 1024
    print(f'Built lpac {VERSION} ({args.target}) -> {output} [{size:.0f} KiB, sha256 {digest}]')


if __name__ == '__main__':
    main()
