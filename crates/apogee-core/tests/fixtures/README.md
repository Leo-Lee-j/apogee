# YAML Format v1 compatibility fixtures

`yaml-v1-compat.json` contains 41 synthetic fixtures (no real credentials). Model
fixtures were emitted by the previous `serde_yaml_ng 0.10.0` codec in an external
probe; the probe also verified that the legacy reader accepts the new writer's
output. Scalar, anchor, merge-key and duplicate-key fixtures capture the old
reader's behavior. The old parser is **not** a product or test dependency.

The committed tests cover both LF/CRLF and UTF-8 BOM inputs, compare full typed
models, and round-trip them with the new codec. Layout may change; the v1 data
contract and string contents must not. `!include` is intentionally not expanded;
no include, properties, robotics or comments feature is enabled.
