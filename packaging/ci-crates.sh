#!/bin/sh
set -eu
candidate=$1
R3_EXTENDED=${R3_EXTENDED:-false}
export R3_EXTENDED
if [ "$R3_EXTENDED" = true ]; then
    R3_MSRV_CARGO=$(rustup which --toolchain 1.89.0 cargo)
    R3_MSRV_RUSTC=$(rustup which --toolchain 1.89.0 rustc)
else
    R3_MSRV_CARGO=
    R3_MSRV_RUSTC=
fi
R3_PLAYWRIGHT_MODULE="$candidate/source/web/node_modules/@playwright/test/index.mjs"
export R3_MSRV_CARGO R3_MSRV_RUSTC R3_PLAYWRIGHT_MODULE
exec node packaging/check-crates.mjs "$candidate"
