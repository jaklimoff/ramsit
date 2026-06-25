# [0.3.0](https://github.com/jaklimoff/ramsit/compare/v0.2.0...v0.3.0) (2026-06-25)


### Bug Fixes

* **aec:** Release/Acquire on aec_epoch signal for weak-memory correctness ([0662d86](https://github.com/jaklimoff/ramsit/commit/0662d86de56600936d31c8f85332fe24ec3809f7))


### Features

* acoustic echo cancellation via pure-Rust aec3 ([8f80b1a](https://github.com/jaklimoff/ramsit/commit/8f80b1a2e4511b1f8c09a0f242a7f57703b43209))
* **aec:** add aec3 dep behind default 'aec' feature ([399d1e2](https://github.com/jaklimoff/ramsit/commit/399d1e2efcee125847c2a997d480b07e4867df60))
* **aec:** Aec owns aec3 pipeline with render feed + in-place capture cancel ([0c4ba0e](https://github.com/jaklimoff/ramsit/commit/0c4ba0ed1ea3718b7a4b5bb7d7782d4bf16712c8))
* **aec:** i16<->f32 (i16-range) conversion and 480-sample framer ([8dec7b0](https://github.com/jaklimoff/ramsit/commit/8dec7b0f7a9802ecbb549ae4832dbbbe3a881c08))
* **aec:** output callback pushes post-gain render reference when aec_wanted ([c0e7acf](https://github.com/jaklimoff/ramsit/commit/c0e7acfbbd448d61081fb217d7513a742bbc0d08))
* **aec:** pump thread owns aec3 pipeline; engine drives AEC via atomics ([6aeba4a](https://github.com/jaklimoff/ramsit/commit/6aeba4a4f19cc645dfe62de2f25e4e9e7e0f1924))
* **aec:** RenderRef bounded queue for output->pump render handoff ([4dd9349](https://github.com/jaklimoff/ramsit/commit/4dd9349ccc8a7045e9e093b4f822b434030b3917))

# [0.2.0](https://github.com/jaklimoff/ramsit/compare/v0.1.1...v0.2.0) (2026-06-21)


### Bug Fixes

* enable opener open_url command (scope alone left link clicks dead) ([64f8ae7](https://github.com/jaklimoff/ramsit/commit/64f8ae78f4e1db89c6120cbe2e30ac08671c945a))


### Features

* add linkify helper for chat urls ([d5721f7](https://github.com/jaklimoff/ramsit/commit/d5721f7ac71ae51fd22f75819e372ace9300a903))
* clickable links in chat ([897b04d](https://github.com/jaklimoff/ramsit/commit/897b04de03aa02bb97fc87f7c71d41a5c897835b))
* render clickable links in chat bubbles ([f662129](https://github.com/jaklimoff/ramsit/commit/f662129336fc10c35d6c2745a92bbcb00bbe2759))
* wire tauri opener plugin scoped to default urls ([e0dfde2](https://github.com/jaklimoff/ramsit/commit/e0dfde272a0c2b080ea7d7c7ee5d13e7861355b2))

## [0.1.1](https://github.com/jaklimoff/ramsit/compare/v0.1.0...v0.1.1) (2026-06-17)


### Bug Fixes

* **ui:** paint an opaque dark background on Windows ([aabb5cf](https://github.com/jaklimoff/ramsit/commit/aabb5cf511f1737ac678d893a78f3a896d6057c5))
