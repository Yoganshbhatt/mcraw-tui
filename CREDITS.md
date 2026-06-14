# Credits

This project incorporates algorithms, data, and concepts from several open-source
colour science and video processing projects. We gratefully acknowledge their
contributions.

## colour-science / colour

- **Source**: https://github.com/colour-science/colour
- **License**: BSD-3-Clause
- **Used for**: Camera log encoding/decoding functions (S-Log3, V-Log, ARRI
  LogC3, Canon C-Log3, Fujifilm F-Log2, ACEScct, PQ, HLG, DaVinci Intermediate),
  color space conversions, chromatic adaptation.

The following transfer function implementations in `src/color.rs` are adapted
from colour-science/colour:
- Sony S-Log3
- Panasonic V-Log
- ARRI LogC3
- Canon C-Log3
- Fujifilm F-Log2
- ACEScct
- SMPTE ST.2084 (PQ)
- HLG (BT.2100)
- DaVinci Intermediate (Blackmagic Design)

## AgX / Troy Sobotka

- **Source**: https://github.com/sobotka/AgX
- **License**: MIT
- **Used for**: AgX tone mapping pipeline in `src/agx.rs`, including spectral
  gamut mapping, tone scale functions, and the Kraken log curve.

## Apple Log Profile

- **Source**: Apple Log Profile White Paper, September 2023
  (https://developer.apple.com/)
- **Used for**: Apple Log / Apple Log 2 transfer function in `src/agx.rs`
- **Color space**: Rec.2020 (BT.2020) primaries with D65 white point

## CAT16 Chromatic Adaptation Transform

- **Source**: CIE TC 1-90 (2016), "Color Illusion"
- **Used for**: CAT16 cone-response matrices in `src/color.rs`

## motioncam-decoder-rust

- **Source**: https://github.com/Yoganshbhatt/motioncam-decoder-rust
- **License**: Apache-2.0
- **Used for**: Decoding .mcraw container files, frame extraction, metadata
  parsing.

## DCI / SMPTE

- DCI-P3 color space defined in SMPTE RP 431-2
- Rec.2020 / BT.2020 color space defined by ITU-R
- Display P3 color space registered by Apple Inc. with ICC

## Other Camera Manufacturer Log Encodings

- **Sony S-Log3 / S-Gamut3**: Sony Corporation
- **Panasonic V-Log / V-Gamut**: Panasonic Corporation
- **ARRI LogC / AWG3 / AWG4**: ARRI AG
- **Canon C-Log / Cinema Gamut**: Canon Inc.
- **Fujifilm F-Log / F-Gamut**: Fujifilm Corporation
- **Blackmagic Design Film Gen 5 / DaVinci Wide Gamut**: Blackmagic Design
- **RED Log3G10 / RED Wide Gamut**: RED Digital Cinema
- **ACES (AP0 / AP1 / ACEScct)**: Academy of Motion Picture Arts and Sciences

## Rust Dependencies

See `Cargo.toml` for the full list of Rust crate dependencies and their
respective licenses.
