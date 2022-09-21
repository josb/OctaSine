#!/bin/bash

set -e

cargo build --profile "release-debug" -p octasine-vst2-plugin

./scripts/macos/bundle.sh "./target/release-debug/liboctasine.dylib" "OctaSine"
./scripts/macos/install.sh "./tmp/OctaSine.vst"
