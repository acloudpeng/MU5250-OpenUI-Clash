#!/usr/bin/env python3
"""Checks an lpac build from scripts/prepare-lpac.py through its stdio drivers, as the
agent drives it (agent/src/lpac.rs), without a card or network:

- requests arrive in order and the APDU replies are accepted (patch 0003);
- a failed first command still closes the logical channel and exits normally (0008);
- a multi-megabyte reply line is read in well under a second (0009).

Usage: tests/lpac_stdio.py <lpac> [runner...], e.g. `tests/lpac_stdio.py build/lpac/lpac qemu-aarch64`.
"""
import json
import subprocess
import sys
import time

ISD_R = 'a0000005591010ffffffff8900000100'


def run(cmd, answer, args):
    """Runs lpac, answering each request with answer(message); returns (messages, exit code)."""
    p = subprocess.Popen(cmd + args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
                         env={'LPAC_APDU': 'stdio', 'LPAC_HTTP': 'stdio'})
    seen = []
    for line in p.stdout:
        message = json.loads(line)
        seen.append(message)
        reply = answer(message)
        if reply is not None:
            p.stdin.write(json.dumps(reply) + '\n')
            p.stdin.flush()
    return seen, p.wait(timeout=60)


def apdu(data):
    return lambda m: {'type': 'apdu', 'payload': {
        'connect': {'ecode': 0}, 'disconnect': {'ecode': 0}, 'logic_channel_close': {'ecode': 0},
        'logic_channel_open': {'ecode': 1}, 'transmit': {'ecode': 0, 'data': data},
    }[m['payload']['func']]} if m['type'] == 'apdu' else None


def main():
    cmd = sys.argv[2:] + [sys.argv[1]]
    # The card answers "file not found" to the first ES10 command (GetEID).
    seen, code = run(cmd, apdu('6a82'), ['chip', 'info'])
    funcs = [m['payload']['func'] for m in seen if m['type'] == 'apdu']
    assert funcs == ['connect', 'logic_channel_open', 'transmit', 'logic_channel_close', 'disconnect'], funcs
    assert seen[1]['payload']['param'] == ISD_R
    assert seen[2]['payload']['param'].startswith('81e291'), seen[2]  # STORE DATA on channel 1
    result = [m for m in seen if m['type'] == 'lpa']
    assert len(result) == 1 and result[0]['payload']['code'] == -1, result
    assert result[0]['payload']['message'] == 'es10c_get_eid', result
    assert code == 255, f'lpac exit status {code} (a crash leaves the channel open)'

    # A download whose first SM-DP+ reply is 2 MB of hex: lpac must read it at once.
    challenge = 'bf2e1280100000000000000000000000000000000000' + '9000'
    started = {}

    def download(m):
        if m['type'] == 'http':
            started['at'] = time.monotonic()
            return {'type': 'http', 'payload': {'rcode': 200, 'rx': '20' * 1_000_000}}
        if m['type'] == 'apdu' and m['payload']['func'] == 'transmit':
            data = challenge if 'bf2e' in m['payload']['param'] else 'bf2003820302029000'
            return {'type': 'apdu', 'payload': {'ecode': 0, 'data': data}}
        return apdu('9000')(m)
    seen, code = run(cmd, download, ['profile', 'download', '-s', 'smdp.example', '-m', 'TEST'])
    result = [m for m in seen if m['type'] == 'lpa']
    assert result and result[0]['payload']['message'] == 'es9p_initiate_authentication', result
    took = [m for m in seen if m['type'] == 'http'] and time.monotonic() - started['at']
    assert took < (20 if len(cmd) > 1 else 2), f'reading a 2 MB reply took {took:.1f} s'
    assert [m['payload']['func'] for m in seen if m['type'] == 'apdu'][-2:] == ['logic_channel_close', 'disconnect']
    print(f'lpac stdio: requests, error path and a 2 MB reply in {took:.2f} s: OK')


if __name__ == '__main__':
    main()
