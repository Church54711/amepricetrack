<img src="assets/logo.svg" width="420" alt="AmePriceTrack">

Self-hosted price tracker. Paste a product link and it checks the price,
keeps the history, and tells you when it drops.

- Staples, Best Buy, Walmart, Target, Amazon, Kohl's, Harbor Freight,
  Hot Topic, and most other shops
- Alerts for a target price, a drop, or back in stock
- Finds the same product at other stores and compares
- Your own discounts and member prices
- Android app in `android/`

## Run

```sh
cargo run    # http://localhost:8787
```

On Ubuntu: `cargo build --release && sudo deploy/install.sh`

Everything else is in [docs/guide.md](docs/guide.md).

## License

[0BSD](LICENSE)
