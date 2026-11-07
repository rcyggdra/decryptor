//! Probe a 3DS rom: container kind, encryption state, title id, title kind and
//! version — all read straight from the file, no external tools needed.
//!
//! Offsets verified against pyctr and a real retail/homebrew CIA:
//! CIA section offsets align on 0x40; the TMD is
//!   sig(4 + sigsize + pad) + header(0xC4) + info records(0x900) + chunks(0x30 ea)
//! with title_id @ hdr[0x4C], title_version u16BE @ hdr[0x9C],
//! content_count u16BE @ hdr[0x9E], chunk records @ data+0x9C4.
//! Ticket fields: encrypted titlekey @0x1BF, title id @0x1DC, commonkey idx @0x1F1.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use byteorder::{BigEndian, ByteOrder, LittleEndian};
use hex_literal::hex;
use log::debug;

use crate::{cbc_decrypt, gen_iv, NcchHdr, NcsdHdr};

#[cfg(test)]
mod tests {
    use super::*;

    fn expect_probe(path: &str, tid: &str, hashed: bool) {
        let p = std::path::Path::new(path);
        if !p.exists() {
            eprintln!("skipping (file not present): {}", path);
            return;
        }
        let mut pro = probe(p).unwrap();
        assert_eq!(pro.title_id.as_deref(), Some(tid), "{}", path);
        if hashed {
            assert_eq!(pro.encrypted, Some(false));
        }
    }

    #[test]
    fn probe_homebrew_cia() {
        // doomlauncher casa from the sd dump: homebrew, fake-signed, plain
        expect_probe(
            "/home/visa/3DS XL/doomlauncher3ds.cia",
            "000400000FDA3D00",
            true,
        );
    }

    #[test]
    fn probe_released_carts() {
        // encrypted retail cartridge dumps from the backup dir
        expect_probe(
            "/home/visa/Games backup 3DS/0004000000030700_v01.3ds",
            "0004000000030700",
            false,
        );
        expect_probe(
            "/home/visa/Games backup 3DS/000400000005A500_v00.3ds",
            "000400000005A500",
            false,
        );
    }
}

pub const CMNKEYS: [[u8; 16]; 6] = [
    hex!("64c5fd55dd3ad988325baaec5243db98"),
    hex!("4aaa3d0e27d4d728d0b1b433f0f9cbc8"),
    hex!("fbb0ef8cdbb0d8e453cd99344371697f"),
    hex!("25959b7ad0409f72684198ba2ecd7dc6"),
    hex!("7ada22caffc476cc8297a0c7ceeeeebe"),
    hex!("a5051ca1b37dcf3afbcf8cc1edd9ce02"),
];

const MEDIA_UNIT: u64 = 512;
const CHUNK_RECORD_SIZE: u64 = 0x30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TitleKind {
    Game,
    Update,
    Dlc,
    System,
    DsiWare,
    #[default]
    Other,
}

impl TitleKind {
    pub fn label(self) -> &'static str {
        match self {
            TitleKind::Game => "Game",
            TitleKind::Update => "Update",
            TitleKind::Dlc => "DLC",
            TitleKind::System => "System",
            TitleKind::DsiWare => "DSiWare",
            TitleKind::Other => "Other",
        }
    }
}

/// kind from a big-endian title id byte array (e.g. 00 04 00 00 0f da 3d 00)
pub fn tid_kind(tid: &[u8]) -> TitleKind {
    if tid.len() < 8 {
        return TitleKind::Other;
    }
    let hex_id = hex::encode(&tid[0..8]).to_uppercase();
    match &hex_id[0..8] {
        "00040000" => TitleKind::Game,
        "0004000E" => TitleKind::Update,
        "0004008C" => TitleKind::Dlc,
        "00040001" | "00040002" | "00040005" | "00040010" | "0004001B" | "0004009B" | "0004009F"
        | "000400FE" => TitleKind::System,
        "00048004" | "00048005" | "0004800D" | "0004800E" | "0004800F" | "00048104" => {
            TitleKind::DsiWare
        }
        _ => TitleKind::Other,
    }
}

pub fn parse_version_u16(v: u16) -> (u16, u16, u16) {
    ((v >> 10) & 0x3F, (v >> 4) & 0x3F, v & 0xF)
}

#[derive(Debug, Default, Clone)]
pub struct Probe {
    /// "000400000FDA3D00" style
    pub title_id: Option<String>,
    /// None = couldn't tell (degenerate/odd file)
    pub encrypted: Option<bool>,
    pub title_kind: Option<TitleKind>,
    pub title_version: Option<(u16, u16, u16)>,
    /// number of NCCH containers successfully inspected (contents for cia,
    /// partitions for ncsd, 1 for ncch/.cci)
    pub ncch_count: u32,
}

#[derive(Debug)]
pub enum ProbeError {
    Io(std::io::Error),
    NotRom(String),
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeError::Io(e) => write!(f, "io error: {}", e),
            ProbeError::NotRom(s) => write!(f, "not a 3DS rom: {}", s),
        }
    }
}

impl std::error::Error for ProbeError {}

impl From<std::io::Error> for ProbeError {
    fn from(e: std::io::Error) -> Self {
        ProbeError::Io(e)
    }
}

fn roundup(x: u64, a: u64) -> u64 {
    (x + a - 1) & !(a - 1)
}

fn sig_data_offset(sig_type: u32) -> Option<u64> {
    let (sig_size, padding) = match sig_type {
        0x00010000 | 0x00010003 => (0x200, 0x3C),
        0x00010001 | 0x00010004 => (0x100, 0x3C),
        0x00010002 | 0x00010005 => (0x3C, 0x40),
        _ => return None,
    };
    Some(4 + sig_size + padding)
}

struct R {
    f: std::fs::File,
    len: u64,
    tikoff: u64,
}

impl R {
    fn at(&mut self, off: u64) -> Result<&mut Self, std::io::Error> {
        self.f.seek(SeekFrom::Start(off))?;
        Ok(self)
    }

    fn read(&mut self, n: usize) -> Result<Vec<u8>, std::io::Error> {
        let mut v = vec![0u8; n];
        self.f.read_exact(&mut v)?;
        Ok(v)
    }

    fn fits(&self, off: u64, n: u64) -> bool {
        off + n <= self.len
    }

    /// Decrypt the ticket's encrypted title key with the common key.
    fn load_title_key(&mut self) -> Option<[u8; 16]> {
        if !self.fits(self.tikoff, 0x1F5) {
            return None;
        }
        self.at(self.tikoff + 0x1BF).ok()?;
        let mut key = [0u8; 16];
        let raw = self.read(16).ok()?;
        key.copy_from_slice(&raw);
        self.at(self.tikoff + 0x1DC).ok()?;
        let mut tid = [0u8; 16];
        let tid8 = self.read(8).ok()?;
        tid[0..8].copy_from_slice(&tid8);
        self.at(self.tikoff + 0x1F1).ok()?;
        let ck = self.read(1).ok()?[0] as usize;
        if ck >= CMNKEYS.len() {
            return None;
        }
        cbc_decrypt(&CMNKEYS[ck], &tid, &mut key);
        Some(key)
    }
}

/// flags offset inside a 512-byte NCCH header (offset of NcchHdr.flags)
const NCCH_FLAGS_OFF: usize = std::mem::offset_of!(NcchHdr, flags);

/// true → content still needs decryption (no "not encrypted" flag)
fn ncch_wants_decrypt(hdr512: &[u8; 512]) -> Option<bool> {
    if &hdr512[256..260] != b"NCCH" {
        return None;
    }
    let f7 = hdr512[NCCH_FLAGS_OFF + 7];
    Some((f7 & 4) == 0)
}

/// Probe any of the supported containers: .cia, .3ds/.cci (NCSD), .cxi (NCCH)
/// or one of the *.ncch files the dumper writes (single NCCH).
pub fn probe(path: &Path) -> Result<Probe, ProbeError> {
    let mut out = Probe::default();

    let file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    if len < 0x210 {
        return Err(ProbeError::NotRom(format!("only {} bytes", len)));
    }

    let mut r = R { f: file, len, tikoff: 0 };
    // peek the NCSD/NCCH magic at 0x100 (256)
    r.at(256)?;
    let mut magic = [0u8; 4];
    r.f.read_exact(&mut magic)?;

    match &magic {
        b"NCSD" => {
            r.at(0)?;
            let hdr = r.read(512)?;
            let hdr: &NcsdHdr = unsafe { &*(hdr.as_ptr() as *const NcsdHdr) };
            let mut tid = hdr.titleid;
            tid.reverse();
            out.title_id = Some(hex::encode(tid).to_uppercase());
            out.title_kind = Some(tid_kind(&tid));
            // check the first present partition's ncch header for the crypto flag
            for table in &hdr.offset_sizetable {
                if table.offset != 0 {
                    let off = (table.offset as u64) * MEDIA_UNIT;
                    if r.fits(off, 512) && off < len {
                        r.at(off)?;
                        if let Ok(h) = r.read(512) {
                            let b: [u8; 512] = h.try_into().unwrap();
                            if let Some(need) = ncch_wants_decrypt(&b) {
                                out.encrypted = Some(need);
                            } else {
                                out.encrypted = Some(false);
                            }
                            out.ncch_count = 1;
                        }
                    }
                    break;
                }
            }
        }
        b"NCCH" => {
            r.at(0)?;
            let b: [u8; 512] = r.read(512)?.try_into().unwrap();
            let hdr: &NcchHdr = unsafe { &*(b.as_ptr() as *const NcchHdr) };
            let mut tid = hdr.titleid;
            tid.reverse();
            out.title_id = Some(hex::encode(tid).to_uppercase());
            out.title_kind = Some(tid_kind(&tid));
            out.encrypted = ncch_wants_decrypt(&b).or(Some(false));
            out.ncch_count = 1;
        }
        _ => {
            // treat as CIA (works for .cia/.3dz and just about any rom the
            // batch tools accepted)
            probe_cia(&mut r, &mut out)?;
        }
    }

    Ok(out)
}

fn probe_cia(r: &mut R, out: &mut Probe) -> Result<(), ProbeError> {
    r.at(0)?;
    let hdr = r.read(0x20)?;
    let headersize = LittleEndian::read_u32(&hdr[0..4]);
    let cert_size = LittleEndian::read_u32(&hdr[8..12]);
    let tik_size = LittleEndian::read_u32(&hdr[12..16]);
    let tmd_size = LittleEndian::read_u32(&hdr[16..20]);

    if headersize < 0x20 || headersize > 0x10000 {
        return Err(ProbeError::NotRom("bad CIA header size".into()));
    }

    let cachain = roundup(headersize as u64, 64);
    let tikoff = cachain + roundup(cert_size as u64, 64);
    let tmdoff = tikoff + roundup(tik_size as u64, 64);
    let contentoff = tmdoff + roundup(tmd_size as u64, 64);

    r.tikoff = tikoff;
    let title_key = r.load_title_key();

    // --- tmd ---
    let mut tmd_parsed = false;
    let mut votes: Vec<bool> = vec![];
    if r.fits(tmdoff, 4) {
        r.at(tmdoff)?;
        let sig_type = BigEndian::read_u32(&r.read(4)?);
        if let Some(data_off) = sig_data_offset(sig_type) {
            if tmdoff + data_off + 0xC4 <= r.len && tmd_size >= (data_off + 0xC4) as u32 {
                r.at(tmdoff + data_off)?;
                let h = r.read(0xC4)?;
                let mut tid = [0u8; 8];
                tid.copy_from_slice(&h[0x4C..0x54]);
                out.title_id = Some(hex::encode(tid).to_uppercase());
                out.title_kind = Some(tid_kind(&tid));

                let v = BigEndian::read_u16(&h[0x9C..0x9E]);
                if v != 0 {
                    out.title_version = Some(parse_version_u16(v));
                }

                let count = BigEndian::read_u16(&h[0x9E..0xA0]);
                if count > 0 && count <= 1024 {
                    let mut next: u64 = 0;
                    for i in 0..count as u64 {
                        let rec_off = tmdoff + data_off + 0x9C4 + i * CHUNK_RECORD_SIZE;
                        if !r.fits(rec_off, CHUNK_RECORD_SIZE) {
                            break;
                        }
                        r.at(rec_off)?;
                        let rec = r.read(CHUNK_RECORD_SIZE as usize)?;
                        let cidx = BigEndian::read_u16(&rec[4..6]);
                        let ctype = BigEndian::read_u16(&rec[6..8]);
                        let csize = BigEndian::read_u64(&rec[8..16]);
                        if csize < 512 {
                            break;
                        }
                        let coff = contentoff + next;
                        if csize > r.len || coff + 512 > r.len {
                            break;
                        }
                        next += roundup(csize, 64);

                        r.at(coff)?;
                        let mut b512: [u8; 512] = r.read(512)?.try_into().unwrap();
                        if (ctype & 1) != 0 {
                            // encrypted content: needs a working title key to read
                            // the header; if we can't get one the vote is "?" and
                            // we try treating the file as whole-encrypted.
                            if let Some(key) = title_key {
                                cbc_decrypt(&key, &gen_iv(cidx), &mut b512);
                                if &b512[256..260] == b"NCCH" {
                                    votes.push(ncch_wants_decrypt(&b512).unwrap_or(true));
                                    out.ncch_count += 1;
                                    continue;
                                }
                            }
                            debug!("content {} did not show NCCH after titlekey decrypt", cidx);
                            out.encrypted = Some(true);
                            return Ok(());
                        } else {
                            // declared plain: is it really usable?
                            if ncch_wants_decrypt(&b512).is_some() {
                                votes.push(ncch_wants_decrypt(&b512).unwrap_or(true));
                                out.ncch_count += 1;
                            }
                        }
                    }
                }
                tmd_parsed = true;
            }
        }
    }

    if !tmd_parsed {
        // nonstandard homebrew cia with broken tmd: fall back to ticket tid
        if r.fits(tikoff + 0x1DC, 8) {
            r.at(tikoff + 0x1DC)?;
            if let Ok(t) = r.read(8) {
                if t.iter().any(|&b| b != 0) {
                    out.title_id = Some(hex::encode(&t).to_uppercase());
                    out.title_kind = Some(tid_kind(&t));
                }
            }
        }
    }

    if !votes.is_empty() && votes.iter().all(|&b| b) {
        out.encrypted = Some(true);
    } else if !votes.is_empty() {
        out.encrypted = Some(false);
    }
    Ok(())
}
