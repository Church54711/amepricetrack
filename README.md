<img src="assets/logo.svg" width="420" alt="AmePriceTrack">

Self-hosted price tracker. Paste a product link and it checks the price,
keeps the history, and tells you when it drops.

- Staples, Best Buy, Walmart, Target, Amazon, Kohl's, Harbor Freight,
  Hot Topic, and most other shops
- Alerts for a target price, a drop, back in stock, or a buy-by day
- Finds the same product at other stores and compares
- Your own discounts and member prices
- Android app in `android/`

## Install

On Ubuntu:

```sh
curl -fsSL https://raw.githubusercontent.com/Church54711/amepricetrack/master/deploy/install.sh | sudo bash
```

The Android app is `AmePriceTrack.apk` in the
[latest release](https://github.com/Church54711/amepricetrack/releases/latest).

From source: `cargo run`, then open http://localhost:8787.

Everything else is in [docs/guide.md](docs/guide.md).

## License

[0BSD](LICENSE)
