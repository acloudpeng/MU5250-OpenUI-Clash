#!/usr/bin/env python3
"""Download files of a published release and check them against its sha256sums.txt.

Python's HTTP client follows the system proxy settings (Internet Options on Windows)
and verifies TLS against the system certificate store. A file is written only after
its checksum matches, and sha256sums.txt last. Does not contact the modem.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import urllib.error
import urllib.request

LIMIT = 128 * 1024 * 1024


def get(url, limit=LIMIT):
    if not url.startswith('https://'):
        raise SystemExit('Release files must be downloaded over HTTPS')
    request = urllib.request.Request(url, headers={'User-Agent': 'openui-download-release'})
    with urllib.request.urlopen(request, timeout=120) as response:
        data = response.read(limit + 1)
    if len(data) > limit:
        raise SystemExit(f'{url} exceeds {limit // (1024 * 1024)} MiB')
    return data


def download(repo, tag, output, names):
    base = f'https://github.com/{repo}/releases/download/{tag}/'
    listing = get(base + 'sha256sums.txt', 1024 * 1024)
    sums = {match[2]: match[1].lower() for line in listing.decode().splitlines()
            if (match := re.fullmatch(r'([0-9a-fA-F]{64}) [ *](\S+)\s*', line))}
    output.mkdir(parents=True, exist_ok=True)
    for name in names:
        if name not in sums:
            raise SystemExit(f'{name} is not listed in the sha256sums.txt of {tag}')
        print(f'{name}: downloading', flush=True)
        data = get(base + name)
        digest = hashlib.sha256(data).hexdigest()
        if digest != sums[name]:
            raise SystemExit(f'{name} does not match sha256sums.txt (got {digest})')
        staged = output / (name + '.new')
        staged.write_bytes(data)
        staged.replace(output / name)
        print(f'{name}: {len(data) / (1024 * 1024):.1f} MiB, checksum matches', flush=True)
    # As published, byte for byte: write_text would turn its line endings into CRLF on Windows.
    (output / 'sha256sums.txt').write_bytes(listing)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', required=True, help='GitHub repository, owner/name')
    parser.add_argument('--tag', help='release to download from')
    parser.add_argument('--output', help='folder for the files')
    parser.add_argument('--latest', action='store_true', help='print the tag of the latest release instead')
    parser.add_argument('names', nargs='*', help='files to download besides sha256sums.txt')
    args = parser.parse_args()
    if not re.fullmatch(r'[A-Za-z0-9-]+/[A-Za-z0-9._-]+', args.repo):
        parser.error('invalid repository')
    try:
        if args.latest:
            print(json.loads(get(f'https://api.github.com/repos/{args.repo}/releases/latest', 1024 * 1024))['tag_name'])
            return
        if not (args.tag and args.output and args.names):
            parser.error('--tag, --output and file names are required')
        if not re.fullmatch(r'v[0-9][0-9A-Za-z.-]*', args.tag) or not all(re.fullmatch(r'[A-Za-z0-9._-]+', n) for n in args.names):
            parser.error('invalid tag or file name')
        download(args.repo, args.tag, Path(args.output), args.names)
    except (urllib.error.URLError, OSError, ValueError, KeyError) as error:
        raise SystemExit(f'Download failed: {error}')


if __name__ == '__main__':
    main()
