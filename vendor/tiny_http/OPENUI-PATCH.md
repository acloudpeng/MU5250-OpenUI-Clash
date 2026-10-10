# OpenUI patch to tiny_http 0.12.0

This directory is tiny_http 0.12.0 as published on crates.io (MIT OR Apache-2.0; see
LICENSE-MIT and LICENSE-APACHE), library sources only, with these changes. The agent's
API is reachable by any LAN client before authentication, so every per-connection
resource must be bounded. Modified files carry a "Modified for OpenUI" line.

- `src/util/equal_reader.rs`: an unread request body used to be drained by allocating
  its whole remaining `Content-Length` at once, so a request claiming a huge length
  aborted the process. It is now drained in 8 KiB chunks and at most 1 MiB; past that
  the connection is closed (the next request line fails to parse).
- `src/client.rs`: request and header lines are limited to 8 KiB and a request to 100
  header lines (both grew without bound); longer input closes the connection, too many
  headers get 400.
- `src/connection.rs`, `src/lib.rs`: every accepted connection gets 60-second read and
  write timeouts, so a client that stops sending (or vanished without closing) frees
  its threads. This also ends idle kept-alive connections after a minute.

The root `Cargo.toml` applies it with `[patch.crates-io]` and excludes `vendor/` from
the workspace. To drop the patch, remove both entries and this directory.
