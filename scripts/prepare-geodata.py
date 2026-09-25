#!/usr/bin/env python3
"""Download the geo databases the Clash core reads for GEOIP/GEOSITE/IP-ASN rules.

The modem usually cannot reach GitHub on first start, so the data ships with the
deployment and the core never downloads it (geo-auto-update is off). MetaCubeX
rebuilds these files daily without versioned tags; each file is checked against the
.sha256sum published beside it, and the result is recorded in SHA256SUMS for
deploy-components.py. Does not contact or change the modem.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import time
import urllib.request

BASE = 'https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest/'
# Upstream name -> name the core looks for in its home directory.
FILES = {'geoip.metadb': 'geoip.metadb', 'geosite.dat': 'GeoSite.dat'}
ASN = {'GeoLite2-ASN.mmdb': 'ASN.mmdb'}
LIMIT = 32 * 1024 * 1024
MMDB_MARKER = b'\xab\xcd\xefMaxMind.com'


def get(url):
    if not url.startswith('https://'):
        raise SystemExit('Geo data must be downloaded over HTTPS')
    request = urllib.request.Request(url, headers={'User-Agent': 'openui-prepare-geodata'})
    with urllib.request.urlopen(request, timeout=60) as response:
        data = response.read(LIMIT + 1)
    if len(data) > LIMIT:
        raise SystemExit(f'{url} exceeds {LIMIT // (1024 * 1024)} MiB')
    return data


def check(name, data):
    if name.endswith(('.metadb', '.mmdb')) and MMDB_MARKER not in data[-128 * 1024:]:
        raise SystemExit(f'{name} is not a MaxMind database')
    if name.endswith('.dat') and len(data) < 1024:
        raise SystemExit(f'{name} is too small to be a geosite database')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', default='build/mihomo/geo')
    parser.add_argument('--base', default=BASE, help='HTTPS mirror serving the same file names')
    parser.add_argument('--asn', action='store_true', help='also fetch the 12 MiB ASN database (IP-ASN rules)')
    args = parser.parse_args()
    base = args.base if args.base.endswith('/') else args.base + '/'
    wanted = {**FILES, **(ASN if args.asn else {})}
    output = Path(args.output)
    output.mkdir(parents=True, exist_ok=True)
    sums = {}
    for upstream, local in wanted.items():
        published = get(base + upstream + '.sha256sum').decode().split()
        if not published or not re.fullmatch(r'[0-9a-f]{64}', published[0]):
            raise SystemExit(f'No published checksum for {upstream}')
        data = get(base + upstream)
        digest = hashlib.sha256(data).hexdigest()
        if digest != published[0]:
            raise SystemExit(f'{upstream} checksum mismatch: got {digest}, published {published[0]}')
        check(local, data)
        target = output / local
        target.with_suffix(target.suffix + '.new').write_bytes(data)
        target.with_suffix(target.suffix + '.new').replace(target)
        sums[local] = digest
        print(f'{local}: {len(data) / (1024 * 1024):.1f} MiB, sha256 {digest}')
    for stale in set(FILES.values()) | set(ASN.values()):
        if stale not in sums:
            (output / stale).unlink(missing_ok=True)
    (output / 'SHA256SUMS').write_text(''.join(f'{d}  {n}\n' for n, d in sorted(sums.items())))
    (output / 'source.json').write_text(json.dumps(
        {'base': base, 'fetched_at': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()), 'files': sums}, indent=2) + '\n')
    print(f'Geo data ready in {output}')


if __name__ == '__main__':
    main()
