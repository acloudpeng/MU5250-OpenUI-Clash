#!/usr/bin/env python3
"""Build mihomo from the pinned Clash.Meta FlClash commit as a static linux/arm64 binary.

Never builds with GOOS=android: that enables embed mode, which disables config reload.
Applies scripts/mihomo-patches/*.patch (standalone-daemon fixes; see each header).
Uses a local Go toolchain matching GO_VERSION, otherwise the golang Docker image.
Does not contact or change the modem.
"""
import argparse
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

REPO = 'https://github.com/chen08209/Clash.Meta.git'
COMMIT = 'd0c8d443298ce26e292a63f0f1016313dda599ac'
GO_VERSION = '1.26.8'
TAGS = 'with_gvisor,with_low_memory,no_tailscale,no_zerotier,no_easytier,no_fake_tcp'
PATCHES = sorted((Path(__file__).resolve().parent / 'mihomo-patches').glob('*.patch'))
VERSION = f'flclash-{COMMIT[:7]}-openui'
# Reproducible with the pinned commit, tags and Go version; any change must update this.
SHA256 = '330642b16e0fc2fc60d31cf165560a57e2ca46fc3b670f3eb8a2f44496a79f77'
LDFLAGS = f'-s -w -buildid= -X github.com/metacubex/mihomo/constant.Version={VERSION}'
BUILD_ENV = {'CGO_ENABLED': '0', 'GOOS': 'linux', 'GOARCH': 'arm64'}
BUILD_ARGS = ['go', 'build', '-mod=readonly', '-trimpath', '-tags', TAGS, '-ldflags', LDFLAGS, '-o']


def run(args, **kwargs):
    subprocess.run(args, check=True, **kwargs)


def fetch(source):
    run(['git', 'init', '-q', str(source)])
    run(['git', '-C', str(source), 'fetch', '-q', '--depth', '1', REPO, COMMIT])
    run(['git', '-C', str(source), 'checkout', '-q', 'FETCH_HEAD'])
    head = subprocess.run(['git', '-C', str(source), 'rev-parse', 'HEAD'],
                          check=True, capture_output=True, text=True).stdout.strip()
    if head != COMMIT:
        raise SystemExit(f'mihomo source commit mismatch: {head}')
    for patch in PATCHES:
        run(['git', '-C', str(source), 'apply', '--whitespace=nowarn', str(patch)])


def local_go():
    go = shutil.which('go')
    if not go:
        return None
    version = subprocess.run([go, 'env', 'GOVERSION'], capture_output=True, text=True).stdout.strip()
    return go if version.startswith(f'go{GO_VERSION}') else None


def build(source, output, use_docker):
    if not use_docker and local_go():
        run(BUILD_ARGS + [str(output), '.'], cwd=source, env={**os.environ, **BUILD_ENV})
        return
    if not shutil.which('docker'):
        raise SystemExit(f'Need Go {GO_VERSION} or Docker to build mihomo')
    env = [arg for key, value in BUILD_ENV.items() for arg in ('-e', f'{key}={value}')]
    cache = Path.home() / '.cache' / 'openui-mihomo-gomod'
    cache.mkdir(parents=True, exist_ok=True)
    run(['docker', 'run', '--rm', '--user', f'{os.getuid()}:{os.getgid()}',
         '-e', 'HOME=/tmp', '-e', 'GOCACHE=/tmp/gocache', '-e', 'GOMODCACHE=/gomod', *env,
         '-v', f'{source}:/src', '-v', f'{cache}:/gomod', '-w', '/src',
         f'golang:{GO_VERSION}', *BUILD_ARGS, '/src/mihomo', '.'])
    shutil.move(source / 'mihomo', output)


def check_elf(path):
    header = path.read_bytes()[:64]
    # ELF64, little endian, e_machine 183 = AArch64
    if header[:5] != b'\x7fELF\x02' or header[18:20] != (183).to_bytes(2, 'little'):
        raise SystemExit('mihomo output is not an aarch64 ELF')
    if b'/lib/ld-' in path.read_bytes()[:4096]:
        raise SystemExit('mihomo output is dynamically linked')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', default='build/mihomo/mihomo')
    parser.add_argument('--docker', action='store_true', help='always build in the golang image')
    parser.add_argument('--no-verify', action='store_true', help='accept a checksum other than SHA256')
    args = parser.parse_args()
    output = Path(args.output).resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='openui-mihomo-') as tmp:
        source = Path(tmp) / 'src'
        fetch(source)
        build(source, output, args.docker)
    check_elf(output)
    output.chmod(0o700)
    digest = hashlib.sha256(output.read_bytes()).hexdigest()
    if digest != SHA256 and not args.no_verify:
        raise SystemExit(f'mihomo checksum {digest} does not match the pinned {SHA256}')
    output.with_name(output.name + '.sha256').write_text(f'{digest}  {output.name}\n')
    size = output.stat().st_size / (1024 * 1024)
    print(f'Built mihomo {VERSION} ({TAGS}) -> {output} [{size:.1f} MiB, sha256 {digest}]')


if __name__ == '__main__':
    main()
