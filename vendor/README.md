# Vendored third-party JavaScript

Pinned copies of runtime dependencies the site serves itself (no CDNs at
runtime — see CLAUDE.md). Every directory carries the upstream license
text; the generated third-party-licenses.html lists them. Update
deliberately, re-record the checksums here, and re-run
tools/gen-third-party-licenses.py.

| Dir | What | Version | Source | License |
|-----|------|---------|--------|---------|
| tfjs/ | TensorFlow.js (core + converter + WebGL/CPU backends, one UMD bundle) | @tensorflow/tfjs 4.22.0 | npm dist/tf.min.js | Apache-2.0 |
| blazeface/ | BlazeFace face detector for tfjs (UMD) + its graph model (model.json + one weight shard, from the tfjs-model hub entry tensorflow/blazeface v1) | @tensorflow-models/blazeface 0.1.0 | npm dist/blazeface.min.umd.js; model via tfhub.dev/tensorflow/tfjs-model/blazeface/1/default/1 | Apache-2.0 |

Only the multiplayer video bubble loads these, lazily, and only when a
player switches their camera on (#200 step 2: the face-tracked crop).

SHA-256 (first 16 hex) at vendoring time, 2026-09-03:
    300dfae273d20b401  tfjs/tf.min.js
    e597d1f48a738cd6b  blazeface/blazeface.min.js
    7b6bb6f35e5a78998  blazeface/model.json
    60b481ab6c193526d  blazeface/group1-shard1of1.bin
