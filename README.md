A multi-threaded `wasm32-unknown-unknown` Web Audio
backend for [Firewheel](https://github.com/BillyDM/firewheel).

Currently, this backend only supports a single, stereo output.
This will be expanded over time.

## Requirements

Because this crate relies on Wasm multi-threading, it has
some additional requirements.

1. A nightly compiler is required along with the Rust standard library source code
   (with `rustup`, you can add it with `rustup component add rust-src`).
2. You'll need the `atomics`, `bulk-memory`, and `mutable-globals` target features.
   These can be enabled with a `.cargo/config.toml`:

```toml
[target.wasm32-unknown-unknown]
rustflags = ["-C", "target-feature=+atomics,+bulk-memory,+mutable-globals"]

[unstable]
build-std = ["std"]
```

3. Wherever your project is served, the protocol must be secure (usually `https`)
   and the response must include two security headers:

```
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
# or
Cross-Origin-Embedder-Policy: credentialless
```

#### License

<sup>
Licensed under either of <a href="LICENSE-APACHE">Apache License, Version
2.0</a> or <a href="LICENSE-MIT">MIT license</a> at your option.
</sup>

<br>

<sub>
Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
</sub>
