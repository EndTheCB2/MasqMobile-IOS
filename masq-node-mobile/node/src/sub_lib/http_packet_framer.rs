// Copyright (c) 2019, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.
use crate::sub_lib::framer::FramedChunk;
use crate::sub_lib::framer::Framer;
use crate::sub_lib::framer_utils;
use crate::sub_lib::utils::to_string;
use masq_lib::logger::Logger;
use masq_lib::utils::index_of;
use masq_lib::utils::index_of_from;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::usize;

pub const MAX_HTTP_HEADER_BYTES: usize = 65_536;

#[derive(Debug, PartialEq, Eq)]
pub enum PacketProgressState {
    SeekingPacketStart,
    SeekingBodyStart,
    SeekingBodyEnd,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ChunkExistenceState {
    Standard,
    ChunkedResponse,
    Chunk,
}

#[derive(PartialEq, Eq, Debug)]
pub enum ChunkProgressState {
    None,
    SeekingLengthHeader,
    SeekingEndOfChunk,
    SeekingEndOfFinalChunk,
}

#[derive(PartialEq, Eq)]
pub struct HttpFramerState {
    pub data_so_far: Vec<u8>,
    pub packet_progress_state: PacketProgressState,
    pub content_length: usize,
    pub transfer_encoding_chunked: ChunkExistenceState,
    pub chunk_progress_state: ChunkProgressState,
    pub chunk_size: Option<usize>,
    pub lines: Vec<Vec<u8>>,
}

impl Debug for HttpFramerState {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "HttpFramerState {{")?;
        writeln!(f, "  data_so_far: {}", to_string(&self.data_so_far))?;
        writeln!(f, "  state: {:?}", self.packet_progress_state)?;
        writeln!(f, "  content_length: {}", self.content_length)?;
        writeln!(
            f,
            "  transfer_encoding_chunked: {:?}",
            self.transfer_encoding_chunked
        )?;
        writeln!(f, "  chunk_progress_state: {:?}", self.chunk_progress_state)?;
        writeln!(f, "  chunk_size: {:?}", self.chunk_size)?;
        writeln!(f, "  lines: [")?;
        for line in &self.lines {
            writeln!(f, "    {}", to_string(line))?;
        }
        writeln!(f, "  ]")?;
        writeln!(f, "}}")
    }
}

pub trait HttpPacketStartFinder: Send {
    fn seek_packet_start(&self, framer_state: &mut HttpFramerState) -> bool;
}

pub struct HttpPacketFramer {
    framer_state: HttpFramerState,
    start_finder: Box<dyn HttpPacketStartFinder>,
    logger: Logger,
}

impl Framer for HttpPacketFramer {
    fn add_data(&mut self, data: &[u8]) {
        self.framer_state.data_so_far.extend(data);
    }

    fn take_frame(&mut self) -> Option<FramedChunk> {
        if self.framer_state.transfer_encoding_chunked == ChunkExistenceState::Chunk {
            self.take_chunk_frame()
        } else {
            self.take_packet_frame()
        }
    }
}

impl HttpPacketFramer {
    pub fn new(start_finder: Box<dyn HttpPacketStartFinder>) -> HttpPacketFramer {
        HttpPacketFramer {
            framer_state: HttpFramerState {
                data_so_far: Vec::new(),
                packet_progress_state: PacketProgressState::SeekingPacketStart,
                content_length: 0,
                transfer_encoding_chunked: ChunkExistenceState::Standard,
                chunk_progress_state: ChunkProgressState::None,
                chunk_size: None,
                lines: Vec::new(),
            },
            start_finder,
            logger: Logger::new("HttpPacketFramer"),
        }
    }

    fn take_packet_frame(&mut self) -> Option<FramedChunk> {
        if self.framer_state.packet_progress_state == PacketProgressState::SeekingPacketStart
            && !self.start_finder.seek_packet_start(&mut self.framer_state)
            || self.framer_state.packet_progress_state == PacketProgressState::SeekingBodyStart
                && !self.seek_body_start()
        {
            return None;
        }
        if self.framer_state.packet_progress_state == PacketProgressState::SeekingBodyEnd {
            self.seek_body_end().map(|request| FramedChunk {
                chunk: request,
                last_chunk: false,
            })
        } else {
            None
        }
    }

    fn seek_body_start(&mut self) -> bool {
        while self.framer_state.packet_progress_state == PacketProgressState::SeekingBodyStart {
            match index_of(&self.framer_state.data_so_far[..], b"\r\n") {
                Some(line_end) => {
                    let line_len = line_end + 2;
                    if self.header_bytes_would_exceed_limit(line_len) {
                        self.discard_oversized_header();
                        return false;
                    }
                    let remainder = self.framer_state.data_so_far.split_off(line_len);
                    let line = self.framer_state.data_so_far.clone();
                    self.framer_state.data_so_far = remainder;
                    if !self.check_for_content_length(&line)
                        || !self.check_for_transfer_encoding(&line)
                    {
                        return false;
                    }
                    let result = self.check_for_zero_length(&line);
                    self.framer_state.lines.push(line);
                    if result {
                        return true;
                    }
                }
                None => {
                    if self.header_bytes_would_exceed_limit(self.framer_state.data_so_far.len()) {
                        self.discard_oversized_header();
                    }
                    return false;
                }
            }
        }
        false
    }

    fn header_bytes_would_exceed_limit(&self, additional_len: usize) -> bool {
        self.framer_state
            .lines
            .iter()
            .try_fold(0usize, |total, line| total.checked_add(line.len()))
            .and_then(|total| total.checked_add(additional_len))
            .map(|total| total > MAX_HTTP_HEADER_BYTES)
            .unwrap_or(true)
    }

    fn discard_oversized_header(&mut self) {
        warning!(
            self.logger,
            "Discarding HTTP packet whose header exceeds {} bytes",
            MAX_HTTP_HEADER_BYTES
        );
        self.framer_state.data_so_far.clear();
        self.framer_state.packet_progress_state = PacketProgressState::SeekingPacketStart;
        self.framer_state.content_length = 0;
        self.framer_state.transfer_encoding_chunked = ChunkExistenceState::Standard;
        self.framer_state.chunk_progress_state = ChunkProgressState::None;
        self.framer_state.chunk_size = None;
        self.framer_state.lines.clear();
    }

    fn seek_body_end(&mut self) -> Option<Vec<u8>> {
        if self.framer_state.packet_progress_state != PacketProgressState::SeekingBodyEnd {
            return None;
        }
        let has_header = !self.framer_state.lines.is_empty();
        let body_bytes_to_take = self
            .framer_state
            .data_so_far
            .len()
            .min(self.framer_state.content_length);
        if !has_header && body_bytes_to_take == 0 {
            return None;
        }
        let remainder = self.framer_state.data_so_far.split_off(body_bytes_to_take);
        let body = std::mem::replace(&mut self.framer_state.data_so_far, remainder);
        self.framer_state.content_length -= body.len();

        let mut request = Vec::with_capacity(
            self.framer_state
                .lines
                .iter()
                .map(Vec::len)
                .sum::<usize>()
                .saturating_add(body.len()),
        );
        for line in self.framer_state.lines.drain(..) {
            request.extend(line);
        }
        request.extend(body);
        if has_header {
            info!(self.logger, "{}", summarize_http_packet(&request));
        }

        if self.framer_state.content_length == 0 {
            self.framer_state.packet_progress_state = PacketProgressState::SeekingPacketStart;
            if self.framer_state.transfer_encoding_chunked == ChunkExistenceState::ChunkedResponse {
                self.framer_state.transfer_encoding_chunked = ChunkExistenceState::Chunk;
                self.framer_state.chunk_progress_state = ChunkProgressState::SeekingLengthHeader;
                self.framer_state.chunk_size = None;
            } else {
                self.framer_state.transfer_encoding_chunked = ChunkExistenceState::Standard;
            }
        }
        Some(request)
    }

    fn check_for_content_length(&mut self, line: &[u8]) -> bool {
        let value = match Self::header_value(line, b"Content-Length") {
            Some(value) => value,
            None => return true,
        };
        if self
            .framer_state
            .lines
            .iter()
            .any(|previous_line| Self::header_value(previous_line, b"Content-Length").is_some())
        {
            self.discard_current_request();
            return false;
        }
        if self.framer_state.transfer_encoding_chunked != ChunkExistenceState::Standard {
            return true;
        }
        let length_str = match std::str::from_utf8(value) {
            Ok(length_str) => length_str,
            Err(_) => {
                self.discard_current_request();
                return false;
            }
        };
        self.framer_state.content_length = match length_str.parse::<usize>() {
            Ok(length) => length,
            Err(_) => {
                self.discard_current_request();
                return false;
            }
        };
        true
    }

    fn check_for_transfer_encoding(&mut self, line: &[u8]) -> bool {
        let value = match Self::header_value(line, b"Transfer-Encoding") {
            Some(value) => value,
            None => return true,
        };
        let encodings = match std::str::from_utf8(value) {
            Ok(encodings) => encodings,
            Err(_) => {
                self.discard_current_request();
                return false;
            }
        };
        if encodings
            .split(',')
            .any(|encoding| encoding.trim().eq_ignore_ascii_case("chunked"))
        {
            self.framer_state.content_length = 0;
            self.framer_state.transfer_encoding_chunked = ChunkExistenceState::ChunkedResponse;
            self.framer_state.chunk_progress_state = ChunkProgressState::SeekingLengthHeader;
            self.framer_state.chunk_size = None;
        }
        true
    }

    fn header_value<'a>(line: &'a [u8], header_name: &[u8]) -> Option<&'a [u8]> {
        let colon = line.iter().position(|byte| *byte == b':')?;
        if !line[..colon].eq_ignore_ascii_case(header_name) {
            return None;
        }
        let mut value = &line[(colon + 1)..];
        if value.ends_with(CRLF) {
            value = &value[..(value.len() - CRLF.len())];
        }
        while matches!(value.first(), Some(b' ' | b'\t')) {
            value = &value[1..];
        }
        while matches!(value.last(), Some(b' ' | b'\t')) {
            value = &value[..(value.len() - 1)];
        }
        Some(value)
    }

    fn check_for_zero_length(&mut self, line: &[u8]) -> bool {
        if line.len() != 2 {
            return false;
        }
        self.framer_state.packet_progress_state = PacketProgressState::SeekingBodyEnd;
        true
    }

    fn discard_current_request(&mut self) {
        self.framer_state.packet_progress_state = PacketProgressState::SeekingPacketStart;
        self.framer_state.content_length = 0;
        self.framer_state.transfer_encoding_chunked = ChunkExistenceState::Standard;
        self.framer_state.chunk_progress_state = ChunkProgressState::None;
        self.framer_state.chunk_size = None;
        self.framer_state.lines.clear();
    }

    fn take_chunk_frame(&mut self) -> Option<FramedChunk> {
        match self.framer_state.chunk_progress_state {
            ChunkProgressState::None => {
                panic!("This should have been set only if we were done reading chunks")
            }
            ChunkProgressState::SeekingLengthHeader => {
                self.take_frame_while_seeking_length_header()
            }
            ChunkProgressState::SeekingEndOfChunk => self.take_frame_while_seeking_end_of_chunk(),
            ChunkProgressState::SeekingEndOfFinalChunk => {
                self.take_frame_while_seeking_end_of_final_chunk()
            }
        }
    }

    fn take_frame_while_seeking_length_header(&mut self) -> Option<FramedChunk> {
        match framer_utils::find_chunk_offset_length(&self.framer_state.data_so_far[..]) {
            None => {
                if self.framer_state.data_so_far.len() > MAX_HTTP_HEADER_BYTES {
                    self.discard_oversized_header();
                } else if let Some(offset) = framer_utils::find_incomplete_chunk_header_offset(
                    &self.framer_state.data_so_far,
                ) {
                    self.framer_state.data_so_far = self.framer_state.data_so_far.split_off(offset);
                } else if self.framer_state.data_so_far.len() > BYTES_TO_PRESERVE {
                    let split = self.framer_state.data_so_far.len() - BYTES_TO_PRESERVE;
                    self.framer_state.data_so_far = self.framer_state.data_so_far.split_off(split);
                }
                None
            }
            Some(chunk_offset_length) => {
                self.framer_state.data_so_far = self
                    .framer_state
                    .data_so_far
                    .split_off(chunk_offset_length.offset);
                if chunk_offset_length.chunk_size == 0 {
                    self.framer_state.chunk_progress_state =
                        ChunkProgressState::SeekingEndOfFinalChunk;
                    self.framer_state.chunk_size = None;
                } else {
                    self.framer_state.chunk_progress_state = ChunkProgressState::SeekingEndOfChunk;
                    self.framer_state.chunk_size = Some(chunk_offset_length.length);
                }
                self.take_chunk_frame()
            }
        }
    }

    fn take_frame_while_seeking_end_of_chunk(&mut self) -> Option<FramedChunk> {
        let chunk_size = self
            .framer_state
            .chunk_size
            .expect("If we are seeking the end of the chunk then we should have the chunk size");
        let complete_chunk_size_opt = chunk_size.checked_add(CRLF.len());
        if !complete_chunk_size_opt
            .map(|complete_chunk_size| self.framer_state.data_so_far.len() >= complete_chunk_size)
            .unwrap_or(false)
        {
            if chunk_size == 0 || self.framer_state.data_so_far.is_empty() {
                return None;
            }
            let bytes_to_take = self.framer_state.data_so_far.len().min(chunk_size);
            let remainder = self.framer_state.data_so_far.split_off(bytes_to_take);
            let chunk = std::mem::replace(&mut self.framer_state.data_so_far, remainder);
            self.framer_state.chunk_size = Some(chunk_size - chunk.len());
            return Some(FramedChunk {
                chunk,
                last_chunk: false,
            });
        }
        let complete_chunk_size = complete_chunk_size_opt.expect("Chunk size disappeared");
        let remaining_data = self.framer_state.data_so_far.split_off(complete_chunk_size);
        let mut chunk = self.framer_state.data_so_far.clone();
        self.framer_state.data_so_far = remaining_data;
        if !chunk.ends_with(CRLF) {
            // If the chunk has no CRLF terminator, rescue the last two characters back into data_so_far
            // Should we consider aborting malformed data-stream?
            self.framer_state
                .data_so_far
                .insert(0, chunk[chunk.len() - 1]);
            self.framer_state
                .data_so_far
                .insert(0, chunk[chunk.len() - 2]);
            let result_data_len = chunk.len();
            chunk.truncate(result_data_len - 2);
        }
        self.framer_state.chunk_progress_state = ChunkProgressState::SeekingLengthHeader;
        self.framer_state.chunk_size = None;
        if chunk.is_empty() {
            self.take_chunk_frame()
        } else {
            Some(FramedChunk {
                chunk,
                last_chunk: false,
            })
        }
    }

    fn take_frame_while_seeking_end_of_final_chunk(&mut self) -> Option<FramedChunk> {
        match index_of(&self.framer_state.data_so_far[..], DOUBLE_CRLF) {
            Some(offset) => {
                let temp = self
                    .framer_state
                    .data_so_far
                    .split_off(offset + DOUBLE_CRLF.len());
                let result_data = self.framer_state.data_so_far.clone();
                self.framer_state.data_so_far = temp;
                self.framer_state.transfer_encoding_chunked = ChunkExistenceState::Standard;
                self.framer_state.chunk_progress_state = ChunkProgressState::None;
                self.framer_state.chunk_size = None;
                Some(FramedChunk {
                    chunk: result_data,
                    last_chunk: false,
                })
            }
            None => {
                if self.framer_state.data_so_far.len() > MAX_HTTP_HEADER_BYTES {
                    self.discard_oversized_header();
                }
                None
            }
        }
    }
}

const BYTES_TO_PRESERVE: usize = 9;
const CRLF: &[u8; 2] = b"\r\n";
const DOUBLE_CRLF: &[u8; 4] = b"\r\n\r\n";

pub fn summarize_http_packet(request: &[u8]) -> String {
    let first_space_index = match index_of_from(request, &(b' '), 0) {
        None => return String::from("<bad HTTP syntax: no spaces>"),
        Some(index) => index,
    };
    let second_space_index = match index_of_from(request, &(b' '), first_space_index + 1) {
        None => return String::from("<bad HTTP syntax: one space>"),
        Some(index) => index,
    };
    let first_token = match std::str::from_utf8(&request[0..first_space_index]) {
        Err(_) => return String::from("<bad HTTP syntax: UTF-8 encoding error>"),
        Ok(token) => token,
    };
    let second_token =
        match std::str::from_utf8(&request[(first_space_index + 1)..second_space_index]) {
            Err(_) => return String::from("<bad HTTP syntax: UTF-8 encoding error>"),
            Ok(token) => token,
        };
    if first_token.starts_with("HTTP/") {
        format!("{} {}", first_token, second_token)
    } else {
        format!("{} [target redacted]", first_token)
    }
}

#[cfg(test)]
mod framer_tests {
    use super::*;
    use crate::sub_lib::http_response_start_finder::HttpResponseStartFinder;
    use crate::sub_lib::utils::to_string;
    use crate::sub_lib::utils::to_string_s;

    #[test]
    fn constants_have_correct_values() {
        assert_eq!(MAX_HTTP_HEADER_BYTES, 65_536);
        assert_eq!(BYTES_TO_PRESERVE, 9);
        assert_eq!(CRLF, b"\r\n");
        assert_eq!(DOUBLE_CRLF, b"\r\n\r\n");
    }

    const GOOD_FIRST_LINE: [u8; 15] = *b"GOOD_FIRST_LINE";

    struct TameStartFinder {}

    impl HttpPacketStartFinder for TameStartFinder {
        fn seek_packet_start(&self, framer_state: &mut HttpFramerState) -> bool {
            if framer_state.packet_progress_state == PacketProgressState::SeekingPacketStart {
                match index_of(&framer_state.data_so_far[..], &GOOD_FIRST_LINE[..]) {
                    Some(offset) => {
                        framer_state.data_so_far = framer_state.data_so_far.split_off(offset);
                        framer_state.packet_progress_state = PacketProgressState::SeekingBodyStart;
                        framer_state.content_length = 0;
                        framer_state.transfer_encoding_chunked = ChunkExistenceState::Standard;
                        framer_state.chunk_progress_state = ChunkProgressState::None;
                        framer_state.chunk_size = None;
                        framer_state.lines.clear();
                        true
                    }
                    None => false,
                }
            } else {
                false
            }
        }
    }

    #[test]
    fn tame_start_finder_yes_clean() {
        let mut framer_state = HttpFramerState {
            data_so_far: Vec::from(&b"GOOD_FIRST_LINE\r\n"[..]),
            packet_progress_state: PacketProgressState::SeekingPacketStart,
            content_length: 100,
            transfer_encoding_chunked: ChunkExistenceState::ChunkedResponse,
            chunk_progress_state: ChunkProgressState::SeekingEndOfFinalChunk,
            chunk_size: Some(200),
            lines: vec![vec![], vec![]],
        };
        let subject = TameStartFinder {};

        let result = subject.seek_packet_start(&mut framer_state);

        assert_eq!(result, true);
        assert_eq!(
            framer_state,
            HttpFramerState {
                data_so_far: Vec::from(&b"GOOD_FIRST_LINE\r\n"[..]),
                packet_progress_state: PacketProgressState::SeekingBodyStart,
                content_length: 0,
                transfer_encoding_chunked: ChunkExistenceState::Standard,
                chunk_progress_state: ChunkProgressState::None,
                chunk_size: None,
                lines: vec![],
            }
        );
    }

    #[test]
    fn tame_start_finder_yes_garbage() {
        let mut framer_state = HttpFramerState {
            data_so_far: Vec::from(&b"garbageGOOD_FIRST_LINE\r\n"[..]),
            packet_progress_state: PacketProgressState::SeekingPacketStart,
            content_length: 100,
            transfer_encoding_chunked: ChunkExistenceState::ChunkedResponse,
            chunk_progress_state: ChunkProgressState::SeekingEndOfFinalChunk,
            chunk_size: Some(200),
            lines: vec![vec![], vec![]],
        };
        let subject = TameStartFinder {};

        let result = subject.seek_packet_start(&mut framer_state);

        assert_eq!(result, true);
        assert_eq!(
            framer_state,
            HttpFramerState {
                data_so_far: Vec::from(&b"GOOD_FIRST_LINE\r\n"[..]),
                packet_progress_state: PacketProgressState::SeekingBodyStart,
                content_length: 0,
                transfer_encoding_chunked: ChunkExistenceState::Standard,
                chunk_progress_state: ChunkProgressState::None,
                chunk_size: None,
                lines: vec![],
            }
        );
    }

    #[test]
    fn tame_start_finder_no_state() {
        let mut framer_state = HttpFramerState {
            data_so_far: Vec::from(&b"GOOD_FIRST_LINE\r\n"[..]),
            packet_progress_state: PacketProgressState::SeekingBodyEnd,
            content_length: 100,
            transfer_encoding_chunked: ChunkExistenceState::ChunkedResponse,
            chunk_progress_state: ChunkProgressState::SeekingEndOfFinalChunk,
            chunk_size: Some(200),
            lines: vec![vec![], vec![]],
        };
        let subject = TameStartFinder {};

        let result = subject.seek_packet_start(&mut framer_state);

        assert_eq!(result, false);
        assert_eq!(
            framer_state,
            HttpFramerState {
                data_so_far: Vec::from(&b"GOOD_FIRST_LINE\r\n"[..]),
                packet_progress_state: PacketProgressState::SeekingBodyEnd,
                content_length: 100,
                transfer_encoding_chunked: ChunkExistenceState::ChunkedResponse,
                chunk_progress_state: ChunkProgressState::SeekingEndOfFinalChunk,
                chunk_size: Some(200),
                lines: vec![vec![], vec![]],
            }
        );
    }

    #[test]
    fn tame_start_finder_no_match() {
        let mut framer_state = HttpFramerState {
            data_so_far: Vec::from(&b"BAD_FIRST_LINE\r\n"[..]),
            packet_progress_state: PacketProgressState::SeekingPacketStart,
            content_length: 100,
            transfer_encoding_chunked: ChunkExistenceState::ChunkedResponse,
            chunk_progress_state: ChunkProgressState::SeekingEndOfFinalChunk,
            chunk_size: Some(200),
            lines: vec![vec![], vec![]],
        };
        let subject = TameStartFinder {};

        let result = subject.seek_packet_start(&mut framer_state);

        assert_eq!(result, false);
        assert_eq!(
            framer_state,
            HttpFramerState {
                data_so_far: Vec::from(&b"BAD_FIRST_LINE\r\n"[..]),
                packet_progress_state: PacketProgressState::SeekingPacketStart,
                content_length: 100,
                transfer_encoding_chunked: ChunkExistenceState::ChunkedResponse,
                chunk_progress_state: ChunkProgressState::SeekingEndOfFinalChunk,
                chunk_size: Some(200),
                lines: vec![vec![], vec![]],
            }
        );
    }

    #[test]
    fn returns_none_if_no_data_has_been_added() {
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));

        let result = subject.take_frame();

        assert_eq!(result, None);
    }

    #[test]
    fn oversized_incomplete_header_is_discarded_and_the_framer_recovers() {
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.add_data(b"GOOD_FIRST_LINE\r\n");
        assert_eq!(subject.take_frame(), None);
        assert_eq!(subject.framer_state.lines.len(), 1);
        subject.add_data(&vec![b'a'; MAX_HTTP_HEADER_BYTES]);

        assert_eq!(subject.take_frame(), None);
        assert!(subject.framer_state.data_so_far.is_empty());
        assert!(subject.framer_state.lines.is_empty());
        assert_eq!(
            subject.framer_state.packet_progress_state,
            PacketProgressState::SeekingPacketStart
        );

        let valid_packet = b"GOOD_FIRST_LINE\r\nContent-Length: 0\r\n\r\n";
        subject.add_data(valid_packet);
        assert_eq!(subject.take_frame().unwrap().chunk, valid_packet);
    }

    #[test]
    fn recognizes_packet_with_body() {
        let request = "GOOD_FIRST_LINE\r\n\
                       One-Header: value\r\n\
                       Content-Length: 26\r\n\
                       Another-Header: value\r\n\
                       \r\n\
                       name=Billy&value=obnoxious"
            .as_bytes();
        let mut data = Vec::from(request);
        data.append(&mut Vec::from("egabrag egabrag".as_bytes()));
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));

        subject.add_data(&data[..]);
        let result = subject.take_frame().unwrap();

        assert_eq!(to_string(&result.chunk), to_string_s(request));
        assert_eq!(result.last_chunk, false)
    }

    #[test]
    fn handles_packet_in_two_pieces_divided_in_middle_of_body_with_garbage() {
        let first_piece = "GOOD_FIRST_LINE\r\nContent-Length: 10\r\n\r\nooga-".as_bytes();
        let second_piece = "booga garbage".as_bytes();
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.add_data(first_piece);
        subject.add_data(second_piece);

        let result = subject.take_frame().unwrap();

        assert_eq!(
            to_string(&result.chunk),
            String::from("GOOD_FIRST_LINE\r\nContent-Length: 10\r\n\r\nooga-booga")
        );
        assert_eq!(result.last_chunk, false)
    }

    #[test]
    fn streams_content_length_body_without_waiting_for_the_entire_body() {
        let first_piece = b"GOOD_FIRST_LINE\r\nContent-Length: 10\r\n\r\nooga-";
        let second_piece = b"boogaGOOD_FIRST_LINE\r\nContent-Length: 0\r\n\r\n";
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.add_data(first_piece);

        let first_result = subject.take_frame().unwrap();

        assert_eq!(first_result.chunk, first_piece);
        assert_eq!(subject.framer_state.content_length, 5);
        assert_eq!(
            subject.framer_state.packet_progress_state,
            PacketProgressState::SeekingBodyEnd
        );
        assert!(subject.framer_state.data_so_far.is_empty());
        assert!(subject.framer_state.lines.is_empty());
        assert_eq!(subject.take_frame(), None);

        subject.add_data(second_piece);
        let second_result = subject.take_frame().unwrap();
        let third_result = subject.take_frame().unwrap();

        assert_eq!(second_result.chunk, b"booga");
        assert_eq!(
            third_result.chunk,
            b"GOOD_FIRST_LINE\r\nContent-Length: 0\r\n\r\n"
        );
        assert_eq!(subject.take_frame(), None);
    }

    #[test]
    fn handles_packet_in_two_pieces_divided_in_middle_of_content_length() {
        let first_piece = "GOOD_FIRST_LINE\r\nCont".as_bytes();
        let second_piece = "ent-Length: 10\r\n\r\nooga-booga".as_bytes();
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.add_data(first_piece);
        subject.add_data(second_piece);

        let result = subject.take_frame().unwrap();
        let should_be_none = subject.take_frame();

        assert_eq!(
            to_string(&result.chunk),
            String::from("GOOD_FIRST_LINE\r\nContent-Length: 10\r\n\r\nooga-booga")
        );
        assert_eq!(result.last_chunk, false);
        assert_eq!(should_be_none, None);
    }

    #[test]
    fn content_length_header_name_is_case_insensitive() {
        let data = b"GOOD_FIRST_LINE\r\ncontent-length:\t5 \r\n\r\nbooga";
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.add_data(data);

        assert_eq!(subject.take_frame().unwrap().chunk, data);
        assert_eq!(subject.take_frame(), None);
    }

    #[test]
    fn duplicate_or_non_numeric_content_length_is_rejected() {
        let malformed_packets: Vec<&[u8]> = vec![
            b"GOOD_FIRST_LINE\r\nContent-Length: 5junk\r\n\r\nbooga",
            b"GOOD_FIRST_LINE\r\nContent-Length: 5\r\ncontent-length: 5\r\n\r\nbooga",
        ];
        for malformed_packet in malformed_packets {
            let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
            subject.add_data(malformed_packet);

            assert_eq!(subject.take_frame(), None);
            assert_eq!(
                subject.framer_state.packet_progress_state,
                PacketProgressState::SeekingPacketStart
            );
            assert_eq!(subject.framer_state.content_length, 0);
            assert!(subject.framer_state.lines.is_empty());
        }
    }

    #[test]
    fn chunked_transfer_encoding_overrides_content_length_in_either_order() {
        let packets: Vec<&[u8]> = vec![
            b"GOOD_FIRST_LINE\r\nContent-Length: 999\r\nTransfer-Encoding: ChUnKeD\r\n\r\nB\r\nFirst chunk\r\n0\r\n\r\n",
            b"GOOD_FIRST_LINE\r\nTransfer-Encoding: ChUnKeD\r\nContent-Length: 999\r\n\r\nB\r\nFirst chunk\r\n0\r\n\r\n",
        ];
        for packet in packets {
            let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
            subject.add_data(packet);

            let header = subject.take_frame().unwrap();
            let body_chunk = subject.take_frame().unwrap();
            let final_chunk = subject.take_frame().unwrap();

            assert!(header.chunk.ends_with(DOUBLE_CRLF));
            assert_eq!(body_chunk.chunk, b"B\r\nFirst chunk\r\n");
            assert_eq!(final_chunk.chunk, b"0\r\n\r\n");
            assert_eq!(subject.take_frame(), None);
        }
    }

    #[test]
    fn handles_multiple_packets_with_bodies_in_one_piece() {
        let data = "GOOD_FIRST_LINE\r\nContent-Length: 5\r\n\r\nbooga\
                    GOOD_FIRST_LINE\r\nContent-Length: 5\r\n\r\ngooba"
            .as_bytes();
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.add_data(data);

        let first_result = subject.take_frame().unwrap();
        let second_result = subject.take_frame().unwrap();

        assert_eq!(
            to_string(&first_result.chunk),
            String::from("GOOD_FIRST_LINE\r\nContent-Length: 5\r\n\r\nbooga")
        );
        assert_eq!(first_result.last_chunk, false);
        assert_eq!(
            to_string(&second_result.chunk),
            String::from("GOOD_FIRST_LINE\r\nContent-Length: 5\r\n\r\ngooba")
        );
        assert_eq!(second_result.last_chunk, false)
    }

    #[test]
    fn discards_packet_with_non_utf8_content_length_line() {
        let mut data = Vec::from("GOOD_FIRST_LINE\r\nContent-Length: ".as_bytes());
        data.push(0xFE);
        data.push(0xFF);
        data.append(&mut Vec::from(
            "\r\n\r\nbooga\
             GOOD_FIRST_LINE\r\nContent-Length: 5\r\n\r\ngooba"
                .as_bytes(),
        ));
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.add_data(&data[..]);

        let result = subject.take_frame();

        assert_eq!(result, None);
        assert_eq!(
            to_string(&subject.framer_state.data_so_far),
            "\r\nboogaGOOD_FIRST_LINE\r\nContent-Length: 5\r\n\r\ngooba"
        );
    }

    #[test]
    fn discards_packet_with_non_utf8_transfer_encoding_line() {
        let mut data = Vec::from("GOOD_FIRST_LINE\r\nTransfer-Encoding: ".as_bytes());
        data.push(0xFE);
        data.push(0xFF);
        data.append(&mut Vec::from(
            "\r\n\r\nbooga\
             GOOD_FIRST_LINE\r\nContent-Length: 5\r\n\r\ngooba"
                .as_bytes(),
        ));
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.add_data(&data[..]);

        let result = subject.take_frame();

        assert_eq!(result, None);
        assert_eq!(
            to_string(&subject.framer_state.data_so_far),
            "\r\nboogaGOOD_FIRST_LINE\r\nContent-Length: 5\r\n\r\ngooba"
        );
    }

    #[test]
    fn discards_packet_with_nonnumeric_content_length() {
        let data = "GOOD_FIRST_LINE\r\nContent-Length: booga\r\n\r\nbooga\
                    GOOD_FIRST_LINE\r\nContent-Length: 5\r\n\r\ngooba"
            .as_bytes();
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.add_data(data);

        let result = subject.take_frame();

        assert_eq!(result, None);
        assert_eq!(
            to_string(&subject.framer_state.data_so_far),
            "\r\nboogaGOOD_FIRST_LINE\r\nContent-Length: 5\r\n\r\ngooba"
        );
    }

    #[test]
    fn discards_packet_with_unparseable_content_length() {
        // Content-Length one more than 2^64
        let data = "GOOD_FIRST_LINE\r\nContent-Length: 18446744073709551616\r\n\r\nbooga\
                    GOOD_FIRST_LINE\r\nContent-Length: 5\r\n\r\ngooba"
            .as_bytes();
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.add_data(data);

        let result = subject.take_frame();

        assert_eq!(result, None);
        assert_eq!(
            to_string(&subject.framer_state.data_so_far),
            "\r\nboogaGOOD_FIRST_LINE\r\nContent-Length: 5\r\n\r\ngooba"
        );
    }

    #[test]
    fn transfer_encoding_is_standard_if_not_mentioned() {
        let data = "GOOD_FIRST_LINE\r\nOoga: Booga\r\n\r\n".as_bytes();
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.add_data(data);

        subject.take_frame().unwrap();

        assert_eq!(
            subject.framer_state.transfer_encoding_chunked,
            ChunkExistenceState::Standard
        );
    }

    #[test]
    fn transfer_encoding_is_standard_if_header_is_present_but_does_not_mention_chunked() {
        let data = "GOOD_FIRST_LINE\r\nTransfer-Encoding: goober, whomp, miffle\r\n\r\n".as_bytes();
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.add_data(data);

        subject.take_frame().unwrap();

        assert_eq!(
            subject.framer_state.transfer_encoding_chunked,
            ChunkExistenceState::Standard
        );
    }

    #[test]
    fn transfer_encoding_is_chunked_if_header_is_present_and_mentions_chunked_alone() {
        let data = "GOOD_FIRST_LINE\r\nTransfer-Encoding: goober, chunked, whomp\r\n".as_bytes();
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.add_data(data);

        let result = subject.take_frame();

        assert_eq!(result, None);
        assert_eq!(
            subject.framer_state.transfer_encoding_chunked,
            ChunkExistenceState::ChunkedResponse
        );
    }

    #[test]
    fn transfer_encoding_is_chunked_if_header_is_present_and_mentions_chunked_among_others() {
        let data =
            "GOOD_FIRST_LINE\r\nTransfer-Encoding: goober, chunked, whomp\r\n\r\n".as_bytes();
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.add_data(data);

        subject.take_frame().unwrap();

        assert_eq!(
            subject.framer_state.transfer_encoding_chunked,
            ChunkExistenceState::Chunk
        );
    }

    #[test]
    fn transfer_encoding_does_not_need_spaces_after_commas() {
        let data = "GOOD_FIRST_LINE\r\nTransfer-Encoding: goober,chunked,whomp\r\n\r\n".as_bytes();
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.add_data(data);

        subject.take_frame().unwrap();

        assert_eq!(
            subject.framer_state.transfer_encoding_chunked,
            ChunkExistenceState::Chunk
        );
    }

    #[test]
    fn transfer_encoding_is_detected_even_if_split_by_buffers() {
        let data1 = "GOOD_FIRST_LINE\r\nTransfer-Encoding: goober,chun".as_bytes();
        let data2 = "ked,whomp\r\n\r\n".as_bytes();
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.add_data(data1);
        subject.add_data(data2);

        subject.take_frame().unwrap();

        assert_eq!(
            subject.framer_state.transfer_encoding_chunked,
            ChunkExistenceState::Chunk
        );
    }

    #[test]
    fn transfer_encoding_response_followed_by_non_chunked_response() {
        let data = "GOOD_FIRST_LINE\r\nTransfer-Encoding: chunked\r\n\r\nB\r\nFirst chunk\r\nC\r\nSecond chunk\r\n0\r\n\r\nGOOD_FIRST_LINE\r\n\r\n".as_bytes();
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.add_data(data);

        let first_response = subject.take_frame().unwrap();
        let first_chunk = subject.take_frame().unwrap();
        let second_chunk = subject.take_frame().unwrap();
        let final_chunk = subject.take_frame().unwrap();
        let second_response = subject.take_frame().unwrap();
        let none = subject.take_frame();

        assert_eq!(
            first_response,
            FramedChunk {
                chunk: Vec::from(
                    "GOOD_FIRST_LINE\r\nTransfer-Encoding: chunked\r\n\r\n".as_bytes()
                ),
                last_chunk: false,
            }
        );
        assert_eq!(
            first_chunk,
            FramedChunk {
                chunk: Vec::from("B\r\nFirst chunk\r\n".as_bytes()),
                last_chunk: false,
            }
        );
        assert_eq!(
            second_chunk,
            FramedChunk {
                chunk: Vec::from("C\r\nSecond chunk\r\n".as_bytes()),
                last_chunk: false,
            }
        );
        assert_eq!(
            final_chunk,
            FramedChunk {
                chunk: Vec::from("0\r\n\r\n".as_bytes()),
                last_chunk: false,
            }
        );
        assert_eq!(
            second_response,
            FramedChunk {
                chunk: Vec::from("GOOD_FIRST_LINE\r\n\r\n".as_bytes()),
                last_chunk: false,
            }
        );
        assert_eq!(none, None);
    }

    #[test]
    fn summarize_http_packethandles_no_spaces() {
        let request = Vec::from("therearenospacesinthisbuffer\r\n".as_bytes());

        let result = summarize_http_packet(&request);

        assert_eq!(result, String::from("<bad HTTP syntax: no spaces>"))
    }

    #[test]
    fn summarize_http_packethandles_single_space() {
        let request = Vec::from("thereisone spaceinthisbuffer\r\n".as_bytes());

        let result = summarize_http_packet(&request);

        assert_eq!(result, String::from("<bad HTTP syntax: one space>"))
    }

    #[test]
    fn summarize_http_packethandles_non_utf8() {
        let request = vec![1, 2, 3, 32, 192, 193, 32, 4, 5];

        let result = summarize_http_packet(&request);

        assert_eq!(
            result,
            String::from("<bad HTTP syntax: UTF-8 encoding error>")
        )
    }

    #[test]
    fn summarize_http_packethandles_good_request() {
        let request = Vec::from("OPTION http://somewhere.com HTTP/1.1\r\n".as_bytes());

        let result = summarize_http_packet(&request);

        assert_eq!(result, String::from("OPTION [target redacted]"))
    }

    #[test]
    fn summarize_http_packethandles_good_response() {
        let request = Vec::from("HTTP/1.1 200 OK\r\n".as_bytes());

        let result = summarize_http_packet(&request);

        assert_eq!(result, String::from("HTTP/1.1 200"))
    }

    #[test]
    fn ignores_garbage_except_for_last_nine_chars() {
        let data = &b"these are the times that try men's souls"[..];
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.framer_state.transfer_encoding_chunked = ChunkExistenceState::Chunk;
        subject.framer_state.chunk_progress_state = ChunkProgressState::SeekingLengthHeader;
        subject.add_data(data);

        let result = subject.take_frame();

        assert_eq!(result, None);
        assert_eq!(
            subject.framer_state.data_so_far,
            Vec::from(&b"n's souls"[..])
        );
        assert_eq!(
            subject.framer_state.chunk_progress_state,
            ChunkProgressState::SeekingLengthHeader
        );
        assert_eq!(subject.framer_state.chunk_size, None);
    }

    #[test]
    fn ignores_hexadecimal_data_except_for_last_nine_chars() {
        let data = &b"0123456789ABCDEFEDCBA98765432\r"[..];
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.framer_state.transfer_encoding_chunked = ChunkExistenceState::Chunk;
        subject.framer_state.chunk_progress_state = ChunkProgressState::SeekingLengthHeader;
        subject.add_data(data);

        let result = subject.take_frame();

        assert_eq!(result, None);
        assert_eq!(
            subject.framer_state.data_so_far,
            Vec::from(&b"98765432\r"[..])
        );
        assert_eq!(
            subject.framer_state.chunk_progress_state,
            ChunkProgressState::SeekingLengthHeader
        );
        assert_eq!(subject.framer_state.chunk_size, None);
    }

    #[test]
    fn senses_beginning_properly() {
        let data = &b"garbageFEDCBA98765432\r\nbeginning of content"[..];
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.framer_state.transfer_encoding_chunked = ChunkExistenceState::Chunk;
        subject.framer_state.chunk_progress_state = ChunkProgressState::SeekingLengthHeader;
        subject.add_data(data);

        let result = subject.take_frame();

        assert_eq!(
            result,
            Some(FramedChunk {
                chunk: Vec::from(&b"98765432\r\nbeginning of content"[..]),
                last_chunk: false,
            })
        );
        assert!(subject.framer_state.data_so_far.is_empty());
        assert_eq!(
            subject.framer_state.chunk_progress_state,
            ChunkProgressState::SeekingEndOfChunk
        );
        assert_eq!(subject.framer_state.chunk_size, Some(0x98765432 - 20));
    }

    #[test]
    fn frames_single_chunk() {
        let data = &b"13\r\nnineteen characters\r\n11\r\nanother"[..];
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.framer_state.transfer_encoding_chunked = ChunkExistenceState::Chunk;
        subject.framer_state.chunk_progress_state = ChunkProgressState::SeekingLengthHeader;
        subject.add_data(data);

        let result = subject.take_frame();

        assert_eq!(
            result,
            Some(FramedChunk {
                chunk: Vec::from(&b"13\r\nnineteen characters\r\n"[..]),
                last_chunk: false,
            })
        );
        assert_eq!(
            subject.framer_state.data_so_far,
            Vec::from(&b"11\r\nanother"[..])
        );
        assert_eq!(
            subject.framer_state.chunk_progress_state,
            ChunkProgressState::SeekingLengthHeader
        );
        assert_eq!(subject.framer_state.chunk_size, None);
    }

    #[test]
    fn streams_incomplete_chunk_data_as_it_arrives() {
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.framer_state.transfer_encoding_chunked = ChunkExistenceState::Chunk;
        subject.framer_state.chunk_progress_state = ChunkProgressState::SeekingLengthHeader;
        subject.add_data(b"A\r\n12345");

        let first_result = subject.take_frame().unwrap();

        assert_eq!(first_result.chunk, b"A\r\n12345");
        assert!(subject.framer_state.data_so_far.is_empty());
        assert_eq!(
            subject.framer_state.chunk_progress_state,
            ChunkProgressState::SeekingEndOfChunk
        );
        assert_eq!(subject.framer_state.chunk_size, Some(5));

        subject.add_data(b"67890\r\n0\r\n\r\n");
        let second_result = subject.take_frame().unwrap();
        let final_result = subject.take_frame().unwrap();

        assert_eq!(second_result.chunk, b"67890\r\n");
        assert_eq!(final_result.chunk, b"0\r\n\r\n");
        assert_eq!(subject.take_frame(), None);
    }

    #[test]
    fn streams_a_chunk_header_extension_fragmented_across_reads() {
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.framer_state.transfer_encoding_chunked = ChunkExistenceState::Chunk;
        subject.framer_state.chunk_progress_state = ChunkProgressState::SeekingLengthHeader;
        subject.add_data(b"garbage!A;fragmented=");

        assert_eq!(subject.take_frame(), None);
        assert_eq!(subject.framer_state.data_so_far, b"A;fragmented=");

        subject.add_data(b"yes\r\n123");
        let first_result = subject.take_frame().unwrap();

        assert_eq!(first_result.chunk, b"A;fragmented=yes\r\n123");
        assert_eq!(subject.framer_state.chunk_size, Some(7));

        subject.add_data(b"4567890\r\n0\r\n\r\n");
        assert_eq!(subject.take_frame().unwrap().chunk, b"4567890\r\n");
        assert_eq!(subject.take_frame().unwrap().chunk, b"0\r\n\r\n");
        assert_eq!(subject.take_frame(), None);
    }

    #[test]
    fn frames_chunk_extensions_and_a_leading_zero_final_chunk() {
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.framer_state.transfer_encoding_chunked = ChunkExistenceState::Chunk;
        subject.framer_state.chunk_progress_state = ChunkProgressState::SeekingLengthHeader;
        subject.add_data(b"A;foo=bar\r\n0123456789\r\n0000;done=yes\r\nTrailer: ok\r\n\r\nNEXT");

        let data_chunk = subject.take_frame().unwrap();
        let final_chunk = subject.take_frame().unwrap();

        assert_eq!(data_chunk.chunk, b"A;foo=bar\r\n0123456789\r\n");
        assert_eq!(final_chunk.chunk, b"0000;done=yes\r\nTrailer: ok\r\n\r\n");
        assert_eq!(subject.framer_state.data_so_far, b"NEXT");
        assert_eq!(
            subject.framer_state.chunk_progress_state,
            ChunkProgressState::None
        );
    }

    #[test]
    fn frames_multiple_chunks_even_unterminated_ones() {
        let data1 = &b"13\r\nnineteen characters\r\ntrash trash16\r"[..];
        let data2 = &b"\nanother few characterstrash1"[..];
        let data3 = &b"2\r\nand one"[..];
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.framer_state.transfer_encoding_chunked = ChunkExistenceState::Chunk;
        subject.framer_state.chunk_progress_state = ChunkProgressState::SeekingLengthHeader;
        subject.add_data(data1);
        subject.add_data(data2);
        subject.add_data(data3);

        let result1 = subject.take_frame();
        let result2 = subject.take_frame();
        let result3 = subject.take_frame();

        assert_eq!(
            result1,
            Some(FramedChunk {
                chunk: Vec::from(&b"13\r\nnineteen characters\r\n"[..]),
                last_chunk: false,
            })
        );
        // unterminated; will cause error in browser, but that's appropriate
        assert_eq!(
            result2,
            Some(FramedChunk {
                chunk: Vec::from(&b"16\r\nanother few characters"[..]),
                last_chunk: false,
            })
        );
        assert_eq!(
            result3,
            Some(FramedChunk {
                chunk: Vec::from(&b"12\r\nand one"[..]),
                last_chunk: false,
            })
        );
        assert!(subject.framer_state.data_so_far.is_empty());
        assert_eq!(
            subject.framer_state.chunk_progress_state,
            ChunkProgressState::SeekingEndOfChunk
        );
        assert_eq!(subject.framer_state.chunk_size, Some(0x12 - 7));
    }

    #[test]
    fn frames_final_chunk_without_header() {
        let data = &b"0\r\n\r\ngarbage"[..];
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.framer_state.transfer_encoding_chunked = ChunkExistenceState::Chunk;
        subject.framer_state.chunk_progress_state = ChunkProgressState::SeekingLengthHeader;
        subject.add_data(data);

        let result = subject.take_frame();

        assert_eq!(
            result,
            Some(FramedChunk {
                chunk: Vec::from(&b"0\r\n\r\n"[..]),
                last_chunk: false,
            })
        );
        assert_eq!(subject.framer_state.data_so_far, Vec::from(&b"garbage"[..]));
        assert_eq!(
            subject.framer_state.chunk_progress_state,
            ChunkProgressState::None
        );
        assert_eq!(subject.framer_state.chunk_size, None);
    }

    #[test]
    fn frames_final_chunk_after_garbage_without_reusing_the_old_offset() {
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.framer_state.transfer_encoding_chunked = ChunkExistenceState::Chunk;
        subject.framer_state.chunk_progress_state = ChunkProgressState::SeekingLengthHeader;
        subject.add_data(b"trash!0\r\n\r\n");

        let result = subject.take_frame();

        assert_eq!(
            result,
            Some(FramedChunk {
                chunk: Vec::from(&b"0\r\n\r\n"[..]),
                last_chunk: false,
            })
        );
        assert!(subject.framer_state.data_so_far.is_empty());
        assert_eq!(
            subject.framer_state.chunk_progress_state,
            ChunkProgressState::None
        );
    }

    #[test]
    fn frames_final_chunk_with_header() {
        let data1 = &b"13\r\nnineteen characters0\r\nHeader: "[..];
        let data2 = &b"value\r\n\r\n"[..];
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.framer_state.transfer_encoding_chunked = ChunkExistenceState::Chunk;
        subject.framer_state.chunk_progress_state = ChunkProgressState::SeekingLengthHeader;
        subject.add_data(data1);
        assert_eq!(subject.take_frame().is_some(), true);
        assert_eq!(subject.take_frame().is_none(), true);
        subject.add_data(data2);

        let result = subject.take_frame();

        assert_eq!(
            result,
            Some(FramedChunk {
                chunk: Vec::from(&b"0\r\nHeader: value\r\n\r\n"[..]),
                last_chunk: false,
            })
        );
        assert_eq!(subject.framer_state.data_so_far, Vec::from(&b""[..]));
        assert_eq!(
            subject.framer_state.chunk_progress_state,
            ChunkProgressState::None
        );
        assert_eq!(subject.framer_state.chunk_size, None);
    }

    #[test]
    fn discards_oversized_incomplete_final_chunk_trailer_and_recovers() {
        let mut subject = HttpPacketFramer::new(Box::new(TameStartFinder {}));
        subject.framer_state.transfer_encoding_chunked = ChunkExistenceState::Chunk;
        subject.framer_state.chunk_progress_state = ChunkProgressState::SeekingEndOfFinalChunk;
        let mut oversized_trailer = Vec::from(&b"0\r\nHeader: "[..]);
        oversized_trailer.extend(vec![b'a'; MAX_HTTP_HEADER_BYTES]);
        subject.add_data(&oversized_trailer);

        assert_eq!(subject.take_frame(), None);
        assert!(subject.framer_state.data_so_far.is_empty());
        assert_eq!(
            subject.framer_state.packet_progress_state,
            PacketProgressState::SeekingPacketStart
        );
        assert_eq!(
            subject.framer_state.transfer_encoding_chunked,
            ChunkExistenceState::Standard
        );
        assert_eq!(
            subject.framer_state.chunk_progress_state,
            ChunkProgressState::None
        );

        let valid_packet = b"GOOD_FIRST_LINE\r\n\r\n";
        subject.add_data(valid_packet);
        assert_eq!(subject.take_frame().unwrap().chunk, valid_packet);
    }

    #[test]
    fn version_of_troublesome_proxy_client_test() {
        let data = &b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 29\r\n\r\nUser-agent: *\nDisallow: /deny";
        let mut subject = HttpPacketFramer::new(Box::new(HttpResponseStartFinder {}));
        subject.add_data(&data[0..40]);
        assert_eq!(subject.take_frame().is_none(), true);
        subject.add_data(&data[40..]);

        let result = subject.take_frame();

        let actual_chunk = result.unwrap();
        assert_eq!(to_string(&actual_chunk.chunk), to_string_s(&data[..]));
        assert_eq!(actual_chunk.last_chunk, false);
    }
}
