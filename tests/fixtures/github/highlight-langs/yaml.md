```yaml
# CI
name: CI
on:
  push:
    branches: [main]
  pull_request:

env:
  RUST_BACKTRACE: 1
  DEBUG: true
  RATIO: 0.5

jobs:
  build:
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest]
    steps:
      - uses: actions/checkout@v4
      - name: Test
        run: |
          cargo test --all
          echo "done"
      - name: "Quoted"
        if: github.event_name == 'push'
anchors: &default
  key: value
other:
  <<: *default
empty: ~
```
