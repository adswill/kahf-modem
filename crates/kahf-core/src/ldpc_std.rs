//! IEEE 802.11 base matrix for the LDPC code with N = 648, R = 1/2 (lifting size Z = 27).
//!
//! The parity-check structure is the published IEEE 802.11 (Wi-Fi) LDPC code of
//! length 648 at rate 1/2, as given in the IEEE 802.11 standard's LDPC section.
//! The base matrix values were taken from an open-source implementation:
//!
//!   Project: LDPC (Python package by Yair M.), https://github.com/yairmz/LDPC
//!   File:    ldpc/code_specs/ieee802.11/N648_R12.qc
//!   URL:     https://raw.githubusercontent.com/yairmz/LDPC/master/ldpc/code_specs/ieee802.11/N648_R12.qc
//!   Licence: MIT, Copyright (c) 2021 Yair M. (licence text in the project's LICENSE file)
//!
//! Format of the matrix: 12 block rows by 24 block columns. Column 0..12 are
//! information blocks (K = 324 bits), columns 12..24 are parity blocks.
//! Entry -1 is an all-zero Z x Z block. Entry s >= 0 is the identity block with
//! its 1 in row r at column (r + s) mod Z.
//!
//! The convention was checked against the 300 reference codewords shipped with the
//! same project (tests/test_data/ieee_802_11/encoded_N648_R12.csv): all 300 satisfy
//! every parity check built from this matrix with the convention above.

pub const Z: usize = 27;
/// Block rows (parity checks in units of Z).
pub const MB: usize = 12;
/// Block columns (codeword length in units of Z).
pub const NB: usize = 24;

pub const BASE: &[&[i32]] = &[
    &[
        0, -1, -1, -1, 0, 0, -1, -1, 0, -1, -1, 0, 1, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
    ],
    &[
        22, 0, -1, -1, 17, -1, 0, 0, 12, -1, -1, -1, -1, 0, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1,
    ],
    &[
        6, -1, 0, -1, 10, -1, -1, -1, 24, -1, 0, -1, -1, -1, 0, 0, -1, -1, -1, -1, -1, -1, -1, -1,
    ],
    &[
        2, -1, -1, 0, 20, -1, -1, -1, 25, 0, -1, -1, -1, -1, -1, 0, 0, -1, -1, -1, -1, -1, -1, -1,
    ],
    &[
        23, -1, -1, -1, 3, -1, -1, -1, 0, -1, 9, 11, -1, -1, -1, -1, 0, 0, -1, -1, -1, -1, -1, -1,
    ],
    &[
        24, -1, 23, 1, 17, -1, 3, -1, 10, -1, -1, -1, -1, -1, -1, -1, -1, 0, 0, -1, -1, -1, -1, -1,
    ],
    &[
        25, -1, -1, -1, 8, -1, -1, -1, 7, 18, -1, -1, 0, -1, -1, -1, -1, -1, 0, 0, -1, -1, -1, -1,
    ],
    &[
        13, 24, -1, -1, 0, -1, 8, -1, 6, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 0, 0, -1, -1, -1,
    ],
    &[
        7, 20, -1, 16, 22, 10, -1, -1, 23, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 0, 0, -1, -1,
    ],
    &[
        11, -1, -1, -1, 19, -1, -1, -1, 13, -1, 3, 17, -1, -1, -1, -1, -1, -1, -1, -1, -1, 0, 0, -1,
    ],
    &[
        25, -1, 8, -1, 23, 18, -1, 14, 9, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 0, 0,
    ],
    &[
        3, -1, -1, -1, 16, -1, -1, 2, 25, 5, -1, -1, 1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 0,
    ],
];
