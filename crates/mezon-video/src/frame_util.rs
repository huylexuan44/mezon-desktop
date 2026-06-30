#![allow(dead_code)]

pub(crate) fn tight_row_bytes(width: u32) -> Option<usize> {
    (width as usize).checked_mul(4)
}

pub(crate) fn tight_bgra_len(width: u32, height: u32) -> Option<usize> {
    tight_row_bytes(width)?.checked_mul(height as usize)
}

pub(crate) fn is_valid_bgra_len(width: u32, height: u32, len: usize) -> bool {
    width != 0 && height != 0 && tight_bgra_len(width, height) == Some(len)
}

pub(crate) fn pack_bgra_rows(
    data: &[u8],
    stride: usize,
    width: u32,
    height: u32,
) -> Option<Vec<u8>> {
    let row_bytes = tight_row_bytes(width)?;
    if width == 0 || height == 0 || stride < row_bytes {
        return None;
    }
    let needed = stride.checked_mul(height as usize)?;
    if data.len() < needed {
        return None;
    }
    let mut out = vec![0u8; row_bytes.checked_mul(height as usize)?];
    for (dst_row, src_row) in out
        .chunks_exact_mut(row_bytes)
        .zip(data.chunks_exact(stride))
    {
        dst_row.copy_from_slice(&src_row[..row_bytes]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tight_row_bytes_multiplies_by_four_and_guards_overflow() {
        assert_eq!(tight_row_bytes(0), Some(0));
        assert_eq!(tight_row_bytes(4), Some(16));
        assert_eq!(
            tight_row_bytes(u32::MAX),
            (u32::MAX as usize).checked_mul(4)
        );
    }

    #[test]
    fn is_valid_bgra_len_requires_exact_tight_length_and_nonzero_dims() {
        assert!(is_valid_bgra_len(2, 2, 16));
        assert!(!is_valid_bgra_len(2, 2, 15));
        assert!(!is_valid_bgra_len(2, 2, 17));
        assert!(!is_valid_bgra_len(0, 2, 0));
        assert!(!is_valid_bgra_len(2, 0, 0));
    }

    #[test]
    fn pack_bgra_rows_copies_tightly_packed_input_unchanged() {
        let data: Vec<u8> = (0..16).collect();
        let out = pack_bgra_rows(&data, 8, 2, 2).expect("tight pack");
        assert_eq!(out, data);
    }

    #[test]
    fn pack_bgra_rows_drops_row_padding() {
        let row0 = [10u8, 11, 12, 13, 14, 15, 16, 17];
        let row1 = [20u8, 21, 22, 23, 24, 25, 26, 27];
        let pad = [0xFFu8, 0xFF];
        let mut data = Vec::new();
        data.extend_from_slice(&row0);
        data.extend_from_slice(&pad);
        data.extend_from_slice(&row1);
        data.extend_from_slice(&pad);

        let out = pack_bgra_rows(&data, 10, 2, 2).expect("padded pack");
        assert_eq!(
            out,
            [
                10, 11, 12, 13, 14, 15, 16, 17, 20, 21, 22, 23, 24, 25, 26, 27
            ]
        );
    }

    #[test]
    fn pack_bgra_rows_rejects_stride_smaller_than_row() {
        assert!(pack_bgra_rows(&[0u8; 16], 4, 2, 2).is_none());
    }

    #[test]
    fn pack_bgra_rows_rejects_insufficient_data() {
        assert!(pack_bgra_rows(&[0u8; 8], 8, 2, 2).is_none());
    }

    #[test]
    fn pack_bgra_rows_rejects_zero_dimensions() {
        assert!(pack_bgra_rows(&[0u8; 16], 8, 0, 2).is_none());
        assert!(pack_bgra_rows(&[0u8; 16], 8, 2, 0).is_none());
    }
}
