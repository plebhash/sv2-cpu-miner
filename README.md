# Sv2 CPU Miner

A Sv2 Spec-compliant CPU miner designed as a swiss-army knife for testing.

It is meant to be used not only as a Sv2 Mining Device, but as a generic Sv2 Mining Protocol Client. Essentially, it can emulate a Sv2 Proxy from the perspective of the Sv2 Mining Protocol Server.

## Features

- **Stratum V2 Protocol**: Support for the Stratum V2 mining protocol
- **Multi Channel Support**: Able to open multiple Standard and/or Extended Sv2 Channels (with optional `REQUIRES_STANDARD_JOBS` flag)
- **Group Channel Support**: Correctly routes work across multiple Group Channels on one connection, including regrouping via `SetGroupChannel` message
- **Flexible UX**: Configurable via TOML file and/or `CPU_MINER__*` environment variables, with `RUST_LOG` verbosity control and optional logging to a file
- **Single Submit Mode**: Option to stop mining after first share submission on each Sv2 Channel
- **CPU Throttling**: Configurable CPU usage (1-100%) to prevent system overload
- **Nominal Hashrate Modification**: Option to modify the nominal hashrate on Sv2 Channel opening (useful to test vardiff on server side)
- **Graceful Shutdown**: Proper cleanup on termination signals (e.g.: Ctrl+C, server disconnect)

## Limitations

This is intented to serve as a Sv2 protocol-compliant testing toolkit. 

It is not optimized for performance, as the hashrate is bound to the tokio runtime.

Therefore, optimizing hashrate is out of scope, and bounded/low hashrate is a known limitation.

## Instructions

First, modify `config.toml` with the desired configuration. Then, you can run the Sv2 CPU Miner with:

```
$ cargo run -- -c config.toml
```

Any field in `config.toml` can be overridden with a `CPU_MINER__<FIELD>` environment variable, for example `CPU_MINER__SERVER_ADDR=127.0.0.1:34254`.

Pass `-f <path>` (`--log-file`) to also write the logs to a file.

## License

This project is licensed under the MIT License.
