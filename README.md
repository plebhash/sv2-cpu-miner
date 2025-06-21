# Sv2 CPU Miner

A Stratum V2 (Sv2) CPU miner implementation based on:
- [Stratum V2 Reference Implementation](https://github.com/stratum-mining/stratum)
- [`tower-stratum`](https://github.com/plebhash/tower-stratum)
- [`tokio`](https://tokio.rs)

## Features

- **Stratum V2 Protocol**: Support for the Stratum V2 mining protocol
- **CPU Throttling**: Configurable CPU usage (1-100%) to prevent system overload
- **Multi Channel Support**: Able to open multiple Standard and/or Extended Sv2 Channels
- **Single Submit Mode**: Option to stop mining after first share submission on each Sv2 Channel
- **Nominal Hashrate Modification**: Option to modify the nominal hashrate on Sv2 Channel opening (useful to test vardiff on server side)
- **Graceful Shutdown**: Proper cleanup on termination signals (e.g.: Ctrl+C, server disconnect)

## Limitations

This is intented to serve as a Sv2 protocol-compliant testing toolkit. 

It is not optimized for performance, as the hashrate is bound to the tokio runtime.

## Instructions

First, modify `config.toml` with the desired configuration. Then, you can run the Sv2 CPU Miner with:

```
$ cargo run -- -c config.toml
```

## License

This project is licensed under the MIT License.
