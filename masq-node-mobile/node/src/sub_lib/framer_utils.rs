// Copyright (c) 2019, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.
use masq_lib::utils::index_of;

#[derive(PartialEq, Eq, Debug)]
pub struct ChunkOffsetLength {
    pub offset: usize,
    pub length: usize,
    pub chunk_size: usize,
}

pub const CRLF: &[u8; 2] = b"\r\n";

pub fn find_chunk_offset_length(data_so_far: &[u8]) -> Option<ChunkOffsetLength> {
    // TODO: Optimization: Only look at new-data length + 17 characters maximum
    let mut accumulated_offset = 0;
    loop {
        match find_next_chunk_offset_length(&data_so_far[accumulated_offset..]) {
            Err(0) => return None,
            Err(next_offset) => accumulated_offset += next_offset,
            Ok(result) => {
                return Some(ChunkOffsetLength {
                    offset: result.offset + accumulated_offset,
                    length: result.length,
                    chunk_size: result.chunk_size,
                });
            }
        }
    }
}

fn find_next_chunk_offset_length(data_so_far: &[u8]) -> Result<ChunkOffsetLength, usize> {
    match index_of(data_so_far, CRLF) {
        None => Err(0),
        Some(0) => Err(CRLF.len()),
        Some(crlf_offset) => match parse_chunk_header_line(&data_so_far[..crlf_offset]) {
            Some((offset, chunk_size)) => chunk_size
                .checked_add(crlf_offset - offset)
                .and_then(|length| length.checked_add(CRLF.len()))
                .map(|length| ChunkOffsetLength {
                    offset,
                    length,
                    chunk_size,
                })
                .ok_or(crlf_offset + CRLF.len()),
            None => Err(crlf_offset + CRLF.len()),
        },
    }
}

pub fn find_incomplete_chunk_header_offset(data_so_far: &[u8]) -> Option<usize> {
    let line_start = data_so_far
        .windows(CRLF.len())
        .rposition(|window| window == CRLF)
        .map(|offset| offset + CRLF.len())
        .unwrap_or(0);
    partial_chunk_header_offset(&data_so_far[line_start..]).map(|offset| line_start + offset)
}

fn parse_chunk_header_line(line: &[u8]) -> Option<(usize, usize)> {
    for offset in 0..line.len() {
        let (digits_end, chunk_size) = match parse_chunk_size_at(line, offset) {
            Some(result) => result,
            None => continue,
        };
        if digits_end == line.len()
            || (line[digits_end] == b';' && valid_chunk_extension(&line[digits_end..]))
        {
            return Some((offset, chunk_size));
        }
    }
    None
}

fn partial_chunk_header_offset(line: &[u8]) -> Option<usize> {
    for offset in 0..line.len() {
        let (digits_end, _) = match parse_chunk_size_at(line, offset) {
            Some(result) => result,
            None => continue,
        };
        if digits_end == line.len()
            || (line[digits_end] == b';' && valid_chunk_extension(&line[digits_end..]))
        {
            return Some(offset);
        }
    }
    None
}

fn parse_chunk_size_at(line: &[u8], offset: usize) -> Option<(usize, usize)> {
    let mut index = offset;
    let mut chunk_size = 0usize;
    let mut digit_count = 0usize;
    while index < line.len() {
        let digit = match evaluate_hex_digit(line[index]) {
            Some(digit) => digit,
            None => break,
        };
        if digit_count == 8 {
            return None;
        }
        chunk_size = chunk_size.checked_mul(16)?.checked_add(digit as usize)?;
        digit_count += 1;
        index += 1;
    }
    if digit_count == 0 {
        None
    } else {
        Some((index, chunk_size))
    }
}

fn valid_chunk_extension(extension: &[u8]) -> bool {
    extension.first() == Some(&b';')
        && extension
            .iter()
            .all(|byte| *byte == b'\t' || (0x20..=0x7e).contains(byte))
}

fn evaluate_hex_digit(digit: u8) -> Option<u8> {
    match position_in_range(digit, b'0', b'9') {
        Some(pos) => Some(pos),
        None => match position_in_range(digit, b'A', b'F') {
            Some(pos) => Some(10 + pos),
            None => position_in_range(digit, b'a', b'f').map(|pos| 10 + pos),
        },
    }
}

fn position_in_range(digit: u8, first: u8, last: u8) -> Option<u8> {
    if digit < first {
        return None;
    }
    if digit > last {
        return None;
    }
    Some(digit - first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_have_correct_values() {
        assert_eq!(CRLF, b"\r\n");
    }

    #[test]
    pub fn returns_none_if_no_crlf() {
        let data_so_far = b"no crlf in this data";

        let result = find_chunk_offset_length(data_so_far);

        assert_eq!(result, None);
    }

    #[test]
    pub fn returns_none_if_just_crlf() {
        let data_so_far = b"\r\nWABBLE";

        let result = find_chunk_offset_length(data_so_far);

        assert_eq!(result, None);
    }

    #[test]
    pub fn returns_none_if_crlf_preceded_by_other_than_hexadecimal_digit() {
        let data_so_far = b"text ends with\r\n";

        let result = find_chunk_offset_length(data_so_far);

        assert_eq!(result, None);
    }

    #[test]
    pub fn returns_data_for_single_capital_hexadecimal_digit() {
        let data_so_far = b"GLORF\r\nWABBLE";

        let result = find_chunk_offset_length(data_so_far);

        assert_eq!(
            result,
            Some(ChunkOffsetLength {
                offset: 4,
                length: 15 + 3,
                chunk_size: 15,
            })
        );
    }

    #[test]
    pub fn returns_data_for_eight_capital_hexadecimal_digits() {
        let data_so_far = b"FEDCBA9876543210123456789ABCDEF\r\nWABBLE";

        let result = find_chunk_offset_length(data_so_far);

        assert_eq!(
            result,
            Some(ChunkOffsetLength {
                offset: 23,
                length: 0x89ABCDEF + 10,
                chunk_size: 0x89ABCDEF,
            })
        );
    }

    #[test]
    pub fn returns_data_for_eight_lowercase_hexadecimal_digits() {
        let data_so_far = b"fedcba9876543210123456789abcdef\r\nWABBLE";

        let result = find_chunk_offset_length(data_so_far);

        assert_eq!(
            result,
            Some(ChunkOffsetLength {
                offset: 23,
                length: 0x89ABCDEF + 10,
                chunk_size: 0x89ABCDEF,
            })
        );
    }

    #[test]
    pub fn returns_data_for_hexadecimal_number_hiding_behind_crlf() {
        let data_so_far = b"\r\n glabble 64\r\nWABBLE";

        let result = find_chunk_offset_length(data_so_far);

        assert_eq!(
            result,
            Some(ChunkOffsetLength {
                offset: 11,
                length: 0x64 + 4,
                chunk_size: 0x64,
            })
        );
    }

    #[test]
    pub fn returns_data_for_hexadecimal_number_hiding_behind_multiple_crlfs() {
        let data_so_far = b"\r\n\r\n\r\n\r\n89abcdef\r\n";

        let result = find_chunk_offset_length(data_so_far);

        assert_eq!(
            result,
            Some(ChunkOffsetLength {
                offset: 8,
                length: 0x89ABCDEF + 10,
                chunk_size: 0x89ABCDEF,
            })
        );
    }

    #[test]
    fn parses_chunk_extensions_and_reports_the_decoded_size() {
        let data_so_far = b"trash!A;foo=bar\r\n0123456789\r\n";

        let result = find_chunk_offset_length(data_so_far);

        assert_eq!(
            result,
            Some(ChunkOffsetLength {
                offset: 6,
                length: 21,
                chunk_size: 10,
            })
        );
    }

    #[test]
    fn recognizes_a_zero_chunk_with_leading_zeroes_and_extensions() {
        let data_so_far = b"0000;done=yes\r\n\r\n";

        let result = find_chunk_offset_length(data_so_far);

        assert_eq!(
            result,
            Some(ChunkOffsetLength {
                offset: 0,
                length: 15,
                chunk_size: 0,
            })
        );
    }

    #[test]
    fn preserves_a_fragmented_chunk_extension_from_its_size() {
        let data_so_far = b"garbage!A;fragmented=yes";

        let result = find_incomplete_chunk_header_offset(data_so_far);

        assert_eq!(result, Some(8));
    }
}
