# Evidence: the backup-key suffix is not recoverable from a CN B31 device

Captured 2026-10-10 on a rooted ZTE U60 Pro (MU5250), firmware
`BD_CNMU5250V1.0.0B31`, `CN_ZTE_MU5250V1.0.0B31`.

This backs the claim in [AI-OPERATIONS.md](AI-OPERATIONS.md) §6.5 and the note in
[DEPLOYMENT.md](DEPLOYMENT.md). **No key value appears in this file** — the
suffix was passed to the device through a file, never a command line, and the
file was shredded afterwards.

## The question

`docs/DEPLOYMENT.md` says:

> If the backup-key suffix ever stops working (ZTE rotated it): extract the new
> suffix from `strings /usr/sbin/zte_web` on any rooted unit.

If that holds, a firmware update that rotates the suffix is survivable. If it
does not, a rotation may permanently end unlocking for a unit. So it matters.

## What the device actually looks like

| Fact | Value |
|---|---|
| Web daemon path | **`/usr/bin/zte_web`** (not `/usr/sbin`) |
| Daemon size | 203,155 bytes, ELF64 aarch64 |
| Binary that runs `device_backup_proc` | `/usr/bin/zte_topsw_mc` (10 refs to `back_parameter`) |
| Backup crypto template lives in | `/usr/lib/libztemc.so` |
| Encrypted backup format | `openssl enc -des-ede3-cbc -md sha256`, `Salted__` |
| Password | `<15-digit IMEI><suffix>` — confirmed by a successful decrypt |

## Test 1 — plain strings in the three candidate binaries

| Binary | literal suffix | 7-char prefix | single-byte XOR (all 255 keys) |
|---|---|---|---|
| `zte_web` | absent | absent | no key reproduces it |
| `zte_topsw_mc` | absent | absent | — |
| `libztemc.so` | absent | absent | — |

Also absent: the reversed form, the lowercased form, and distinctive
substrings drawn from the value itself (the sweep in Test 2 used a seven
character prefix for exactly this reason — a full-value search would have been
unnecessary work, and reporting the fragments would leak the key by degrees).

## Test 2 — whole-firmware sweep, on the device

Scanned every regular file (ELF-gated only in the first pass, unrestricted in
the second):

| Sweep | Scope | Files | Matches |
|---|---|---|---|
| ELF sweep | `/bin /sbin /usr/bin /usr/sbin /usr/lib /lib` | **1,524 binaries** | 0 |
| Prefix sweep | same set, 7-char prefix only | 1,524 binaries | 0 |
| Data sweep | `/usr/lib /lib /usr/share /etc_rw` (all types) | all regular files | 0 |
| Targeted | `/etc/config/*`, `*.conf`, `save_uci_list.json`, `tz.db` | — | 0 |

**Zero occurrences of the suffix, or even its first seven characters, anywhere
in the firmware.**

## Test 3 — is it derivable from on-device identifiers?

The binary shells out to `echo -n "<something>" | openssl dgst -sha256`, and
`libztemc.so` references `/proc/zte_unique_id` next to its backup code, so a
derivation is plausible. Tested against a real backup with a known-good
password as an oracle (49 derivations):

- IMEI alone; `/proc/zte_unique_id` alone; both concatenations, both orders
- `md5` / `sha1` / `sha256` of IMEI, of the unique id, of their concatenation,
  in lowercase and uppercase, bare and IMEI-prefixed
- IMEI first/last 4, 6, 8 digits appended
- the one high-entropy literal string found in `zte_web`
  (`Tz*RZGZ5G*D0*tDD^4_dT!4E`, 24 chars) alone and concatenated with the IMEI
- assorted short product-family guesses

**None decrypts the backup.** The oracle itself was verified both ways: the
true password decrypts, a deliberately wrong one is rejected with a padding
error.

## Test 4 — obfuscation

Single-byte XOR over the full 0x01–0xFF keyspace: no key reproduces the suffix.
Reversed and case variants: absent. No compressed or encoded blob search was
performed beyond that, so a multi-byte scheme is not excluded — but nothing in
the binaries suggests one.

## Conclusion

On CN B31:

1. **The documented recovery path does not work** — wrong path, and the string
   is not there.
2. The suffix is **not** a plaintext constant in any firmware file, and **not**
   derivable from the identifiers a user can read.
3. Therefore **a suffix rotation by ZTE must be treated as potentially
   unrecoverable** on a unit that has no independent copy of the new value.

Practical consequence, already folded into the runbooks:

- Capture firmware fingerprints **before** any update
  ([AI-OPERATIONS.md](AI-OPERATIONS.md) §6.2).
- Keep `/data/dropbear/` — it survives FOTA and is what makes post-update
  recovery possible without re-unlocking.
- Do not rely on being able to re-derive a rotated suffix.

## Reproducing

`tools/` in the working notes contained the sweep scripts; they are not shipped
here because they take the key as input. The essential shape:

```sh
# on a rooted unit; read the needle from a FILE, never argv
NEEDLE=$(cat /path/to/needle)          # file mode 600, shred after use
PREFIX=$(echo "$NEEDLE" | cut -c1-7)

for dir in /bin /sbin /usr/bin /usr/sbin /usr/lib /lib; do
    for f in "$dir"/*; do
        [ -f "$f" ] || continue
        head -c 4 "$f" | od -An -tx1 | grep -q '7f 45 4c 46' || continue
        strings "$f" 2>/dev/null | grep -qF "$PREFIX" && echo "FOUND $f"
    done
done
```

> **Never paste a real suffix into a shell command line, a log, or an issue.**
> That is how it gets rotated for everyone.
