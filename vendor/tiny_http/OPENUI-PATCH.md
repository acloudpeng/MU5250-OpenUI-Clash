# OpenUI patch to tiny_http 0.12.0

This directory is tiny_http 0.12.0 as published on crates.io (MIT OR Apache-2.0; see
LICENSE-MIT and LICENSE-APACHE), library sources only, with these changes. The agent's
API is reachable by any LAN client before authentication, so every per-connection
resource must be bounded. Modified files carry a "Modified for OpenUI" line.

- `src/util/equal_reader.rs`: an unread request body used to be drained by allocating
  its whole remaining `Content-Length` at once, so a request claiming a huge length
  aborted the process. It is now drained in 8 KiB chunks and at most 1 MiB.
  The drain runs on the thread that drops the request, one of the agent's four
  handlers, and each read waited up to the connection timeout, renewed by every byte:
  four clients declaring a body and sending none (or a byte now and then) stalled the
  whole API for a minute or more. The drain now reads the socket non-blocking, taking
  only what has already arrived (no read starts after 1 s, as a backstop), so dropping
  a request never waits for the client. A fully drained body, as from a client that
  already sent it, leaves the connection blocking again and reusable for the next
  request; otherwise (nothing more has arrived, size limit, end of input, read error)
  the connection is abandoned and closes once its response is written. The non-blocking
  flag is shared by every handle on the socket; an earlier response still being written
  meanwhile (only with pipelining, which browsers do not use) can fail.
- `src/connection.rs`: `ConnectionControl`, a second handle on an accepted socket (taken
  in `src/lib.rs` before TLS, if any; one more file descriptor per connection) with an
  "abandoned" flag, shared by the connection's requests through `src/client.rs`,
  `src/request.rs` and `src/test.rs` (a `TestRequest` body is in memory and gets none).
  `src/client.rs` refuses every read of an abandoned connection, above its read buffer,
  so the rest of the body is never parsed as requests. The connection's own thread then
  reads and throws away what the client is still sending for up to 2 s (200 ms quiet,
  16 MiB at most) before letting go of the socket, because closing with unread data
  resets the connection and can make the client lose its response, as with an upload
  refused before its body was read. It does not shut down writing: a body that ends
  early is dropped by `FusedReader` while the application is still reading it, before
  the response is written, and the socket closes (FIN after the response) only when
  the request's writer lets go as well; for the same reason only the read timeout is
  shortened meanwhile, so that response keeps its 60-second write timeout. This runs on tiny_http's connection thread, not
  on the application's handlers.
- `src/client.rs`: request and header lines are limited to 8 KiB and a request to 100
  header lines (both grew without bound); longer input closes the connection, too many
  headers get 400. A request line with HTTP/2 or later gets its 505 through the request
  itself; the original wrote it through the next writer, which waits for the one the
  request holds, and so hung the connection thread (and its descriptors) for good.
- `src/connection.rs`, `src/lib.rs`: every accepted connection gets 60-second read and
  write timeouts, so a client that stops sending (or vanished without closing) frees
  its threads. This also ends idle kept-alive connections after a minute.

The root `Cargo.toml` applies it with `[patch.crates-io]` and excludes `vendor/` from
the workspace. To drop the patch, remove both entries and this directory.
