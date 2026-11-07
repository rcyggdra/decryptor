//! ctrdecrypt — 3DS decryption library (extracted from the ctrdecrypt CLI).
//!
//! The binary in `src/bin/cryptdecrypt.rs` is a thin CLI wrapper; everything
//! here is reusable, which lets the TUI drive decryption in-process and get
//! real progress instead of shelling out.

mod ctrutils;
pub mod probe;
pub use ctrutils::{cbc_decrypt, gen_iv, CiaContent, CiaFile, CiaReader, NcchHdr, NcsdHdr};
pub use probe::{probe, Probe, ProbeError, TitleKind};

use byteorder::{ByteOrder, BigEndian, LittleEndian, ReadBytesExt};
use aes::{cipher::{KeyIvInit, StreamCipher}, Aes128};

use std::{collections::HashMap, fs::canonicalize, fs::File, io::{Cursor, Read, Seek, SeekFrom, Write}, path::{Path, PathBuf}, vec};

use hex_literal::hex;
use log::{debug, info, LevelFilter};

const CMNKEYS: [[u8; 16]; 6] = [
    hex!("64c5fd55dd3ad988325baaec5243db98"),
    hex!("4aaa3d0e27d4d728d0b1b433f0f9cbc8"),
    hex!("fbb0ef8cdbb0d8e453cd99344371697f"),
    hex!("25959b7ad0409f72684198ba2ecd7dc6"),
    hex!("7ada22caffc476cc8297a0c7ceeeeebe"),
    hex!("a5051ca1b37dcf3afbcf8cc1edd9ce02")
];

const KEY_0X2C: u128 = 246647523836745093481291640204864831571;
const KEY_0X25: u128 = 275024782269591852539264289417494026995;
const KEY_0X18: u128 = 174013536497093865167571429864564540276;
const KEY_0X1B: u128 = 92615092018138441822550407327763030402;
const FIXED_SYS: u128 = 109645209274529458878270608689136408907;

const KEYS_0: [u128; 4] = [KEY_0X2C, KEY_0X25, KEY_0X18, KEY_0X1B];
const KEYS_1: [u128; 2] = [0, FIXED_SYS];

const NCSD_PARTITIONS: [&str; 8] = [
    "Main",
    "Manual",
    "Download Play",
    "Partition4",
    "Partition5",
    "Partition6",
    "N3DSUpdateData",
    "UpdateData"
];

const MEDIA_UNIT_SIZE: u32 = 512;

pub type Aes128Ctr = ctr::Ctr128BE<Aes128>;

/// Where seeddb.bin lives (the bundled `bin/seeddb.bin` on Linux).
pub const SEEDDB_FALLBACK_DIRS: [&str; 2] = ["$HOME/3ds", "$HOME/.3ds"];

#[derive(Debug)]
pub enum DecryptError {
    Io(std::io::Error),
    /// e.g. "ROM does not exist", "Unsupported CIA file", dumper refuse.
    NotSupported(String),
    Path(String),
}

impl std::fmt::Display for DecryptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecryptError::Io(e) => write!(f, "io error: {}", e),
            DecryptError::NotSupported(s) => write!(f, "not supported: {}", s),
            DecryptError::Path(s) => write!(f, "path: {}", s),
        }
    }
}

impl std::error::Error for DecryptError {}

impl From<std::io::Error> for DecryptError {
    fn from(e: std::io::Error) -> Self {
        DecryptError::Io(e)
    }
}

/// Progress/controls shared through a run.
pub struct Run<'a> {
    /// bytes processed so far (0..total).
    pub done: u64,
    /// total expected bytes (file size).
    pub total: u64,
    pub opts: &'a DecryptOptions,
    /// extracted .ncch files, in dump order.
    pub outputs: Vec<PathBuf>,
}

impl<'a> Run<'a> {
    fn report(&mut self, add: u64) {
        self.done += add;
        if let Some(p) = &self.opts.progress {
            p(self.done, self.total);
        }
    }
}

pub type ProgressFn = dyn Fn(u64, u64) + Send + Sync;

/// Configuration handed to [`decrypt`].
pub struct DecryptOptions {
    /// dump debug lines alongside info (bin passes --no-verbose off).
    pub verbose: bool,
    /// explicit seeddb.bin path; falls back to ~/3ds, ~/.3ds, ./seeddb.bin.
    pub seeddb: Option<PathBuf>,
    /// let ctrdecrypt look up missing seeds online (kagiya servers), like upstream.
    pub allow_online: bool,
    /// only dump this cia content index (upstream --ncch <n>).
    pub partition: Option<u8>,
    /// per-chunk progress callback: (done, total).
    pub progress: Option<Box<ProgressFn>>,
}

impl Default for DecryptOptions {
    fn default() -> Self {
        DecryptOptions {
            verbose: true,
            seeddb: None,
            allow_online: true,
            partition: None,
            progress: None,
        }
    }
}

enum NcchSection {
    ExHeader = 1,
    ExeFS = 2,
    RomFS = 3
}

fn align(x: u64, y: u64) -> u64 {
    let mask: u64 = !(y - 1);
    (x + (y - 1)) & mask
}

fn flag_to_bool(flag: u8) -> bool {
    match flag {
        1..=u8::MAX => true,
        0 => false
    }
}

fn get_ncch_aes_counter(hdr: &NcchHdr, section: NcchSection) -> [u8; 16] {
    let mut counter: [u8; 16] = [0; 16];
    if hdr.formatversion == 2 || hdr.formatversion == 0 {
        let mut titleid: [u8; 8] = hdr.titleid;
        titleid.reverse();
        counter[0..8].copy_from_slice(&titleid);
        counter[8] = section as u8;

    } else if hdr.formatversion == 1 {
        let x = match section {
            NcchSection::ExHeader => 512,
            NcchSection::ExeFS => hdr.exefsoffset * MEDIA_UNIT_SIZE,
            NcchSection::RomFS => hdr.romfsoffset * MEDIA_UNIT_SIZE
        };

        counter[0..8].copy_from_slice(&hdr.titleid);
        for i in 0..4 {
            counter[12 + i] = (x >> ((3 - i) * 8) & 255) as u8
        }
    }

    counter
}

fn scramblekey(key_x: u128, key_y: u128) -> u128 {
    const MAX_BITS: u32 = 128;
    const MASK: u128 = u128::MAX;

    let rol = |val: u128, r_bits: u32| -> u128 {
        let r_bits = r_bits % MAX_BITS; // Ensure the shift is within bounds
        (val << r_bits) | (val >> (MAX_BITS - r_bits))
    };

    let value = (rol(key_x, 2) ^ key_y) + (42503689118608475533858958821215598218 & MASK);
    rol(value, 87)
}

#[allow(clippy::too_many_arguments)]
fn dump_section(ncch: &mut File, cia: &mut CiaReader, offset: u64, size: u32, sec_type: NcchSection, sec_idx: usize, ctr: [u8; 16], uses_extra_crypto: u8, fixed_crypto: u8, use_seed_crypto: bool, encrypted: bool, keyys: [u128; 2], run: &mut Run) {
    let sections = ["ExHeader", "ExeFS", "RomFS"];
    const CHUNK: u32 = 4194304; // 4 MiB
    debug!("  {} offset: {:08X}", sections[sec_idx], offset);
    debug!("  {} counter: {}", sections[sec_idx], hex::encode(&ctr));
    debug!("  {} size: {} bytes", sections[sec_idx], size);

    // Prevent integer overflow
    match offset.checked_sub(ncch.stream_position().unwrap()) {
        Some(tmp) => {
            if tmp > 0 {
                let mut buf = vec![0u8; tmp as usize];
                cia.read(&mut buf);
                run.report(buf.len() as u64);
                if ncch.stream_position().unwrap() == 512 { buf[1] = 0x00; }
                ncch.write_all(&buf).unwrap();
            }
        }
        None => ()
    }

    if !encrypted {
        let mut sizeleft = size;
        let mut buf = vec![0u8; CHUNK as usize];

        while sizeleft > CHUNK {
            cia.read(&mut buf);
            run.report(buf.len() as u64);
            ncch.write_all(&buf).unwrap();
            sizeleft -= CHUNK;
        }

        if sizeleft > 0 {
            buf = vec![0u8; sizeleft as usize];
            cia.read(&mut buf);
            run.report(buf.len() as u64);
            ncch.write_all(&buf).unwrap();
        }
        return;
    }

    let key_0x2c = u128::to_be_bytes(scramblekey(KEYS_0[0], keyys[0]));
    let get_crypto_key = |extra_crypto: &u8| -> usize { match extra_crypto { 0 => 0, 1 => 1, 10 => 2, 11 => 3, _ => 0 }};

    match sec_type {
        NcchSection::ExHeader => {
            let mut key = key_0x2c;
            if flag_to_bool(fixed_crypto) {
                key = u128::to_be_bytes(KEYS_1[(fixed_crypto as usize)- 1]);
            }
            let mut buf = vec![0u8; size as usize];
            cia.read(&mut buf);
            run.report(buf.len() as u64);
            Aes128Ctr::new_from_slices(&key, &ctr)
                .unwrap()
                .apply_keystream(&mut buf);
            ncch.write_all(&buf).unwrap();
        }
        NcchSection::ExeFS => {
            let mut key = key_0x2c;
            if flag_to_bool(fixed_crypto) {
                key = u128::to_be_bytes(KEYS_1[(fixed_crypto as usize)- 1]);
            }
            let mut exedata = vec![0u8; size as usize];
            cia.read(&mut exedata);
            run.report(exedata.len() as u64);
            let mut exetmp = exedata.clone();
            Aes128Ctr::new_from_slices(&key, &ctr)
                .unwrap()
                .apply_keystream(&mut exetmp);

            if flag_to_bool(uses_extra_crypto) || use_seed_crypto {
                let mut exetmp2 = exedata;
                key = u128::to_be_bytes(scramblekey(KEYS_0[get_crypto_key(&uses_extra_crypto)], keyys[1]));

                Aes128Ctr::new_from_slices(&key, &ctr)
                    .unwrap()
                    .apply_keystream(&mut exetmp2);

                #[repr(C)]
                struct ExeInfo {
                    fname: [u8; 8],
                    off: [u8; 4],
                    size: [u8; 4],
                }

                for i in 0usize..10 {
                    let exebytes = &exetmp[i * 16..(i + 1) * 16];
                    let exeinfo: ExeInfo = unsafe { std::mem::transmute(LittleEndian::read_u128(exebytes)) };

                    let mut off = LittleEndian::read_u32(&exeinfo.off) as usize;
                    let size = LittleEndian::read_u32(&exeinfo.size) as usize;
                    off += 512;

                    match exeinfo.fname.iter().rposition(|&x| x != 0) {
                        Some(zero_idx) => if exeinfo.fname[..=zero_idx].is_ascii()
                        {
                            // ASCII for 'icon'
                            let icon: [u8; 4] = hex!("69636f6e");
                            // ASCII for 'banner'
                            let banner: [u8; 6] = hex!("62616e6e6572");

                            if !(exeinfo.fname[..=zero_idx] == icon || exeinfo.fname[..=zero_idx] == banner) {
                                exetmp.splice(off..(off + size), exetmp2[off..off + size].iter().cloned());
                            }
                        }
                        None => { exetmp.splice(off..(off + size), exetmp2[off..off + size].iter().cloned()); }
                    }
                }
            }
            ncch.write_all(&exetmp).unwrap();
        }
        NcchSection::RomFS => {
            let mut key = u128::to_be_bytes(scramblekey(KEYS_0[get_crypto_key(&uses_extra_crypto)], keyys[1]));
            if flag_to_bool(fixed_crypto) {
                key = u128::to_be_bytes(KEYS_1[(fixed_crypto as usize) - 1]);
            }
            let mut sizeleft = size;
            let mut buf = vec![0u8; CHUNK as usize];
            let mut ctr_cipher = Aes128Ctr::new_from_slices(&key, &ctr).unwrap();
            while sizeleft > CHUNK {
                cia.read(&mut buf);
                run.report(buf.len() as u64);
                if cia.cidx > 0 && !(cia.single_ncch || cia.from_ncsd) { buf[1] = buf[1] ^ cia.cidx as u8 }
                ctr_cipher.apply_keystream(&mut buf);
                ncch.write_all(&buf).unwrap();
                sizeleft -= CHUNK;
            }

            if sizeleft > 0 {
                buf = vec![0u8; sizeleft as usize];
                cia.read(&mut buf);
                run.report(buf.len() as u64);
                if cia.cidx > 0 && !(cia.single_ncch || cia.from_ncsd) { buf[1] = buf[1] ^ cia.cidx as u8 }
                ctr_cipher.apply_keystream(&mut buf);
                ncch.write_all(&buf).unwrap();
            }
        }
    }
}

fn get_new_key(key_y: u128, header: &NcchHdr, titleid: String, run: &mut Run) -> u128 {
    let db_path = run.opts.seeddb.clone().unwrap_or_else(|| {
        // upstream default: seeddb.bin in the current directory
        PathBuf::from("seeddb.bin")
    });

    if !db_path.exists() {
        // ihaveacreditcard-style key locations (linux native)
        let home = std::env::var("HOME").unwrap_or_default();
        for cand in ["3ds", ".3ds"] {
            let p = Path::new(&home).join(cand).join("seeddb.bin");
            if p.exists() {
                return get_new_key_with_path(key_y, header, titleid, run, &p);
            }
        }
    }

    get_new_key_with_path(key_y, header, titleid, run, &db_path)
}

fn get_new_key_with_path(key_y: u128, header: &NcchHdr, titleid: String, run: &mut Run, db_path: &Path) -> u128 {
    let mut new_key: u128 = 0;
    let mut seeds: HashMap<String, [u8; 16]> = HashMap::new();

    let seeddb = File::open(db_path);
    let mut cbuffer: [u8; 4] = [0; 4];
    let mut kbuffer: [u8; 8] = [0; 8];
    let mut sbuffer: [u8; 16] = [0; 16];

    // Check into seeddb.bin
    match seeddb {
        Ok(mut seeddb) => {
            match seeddb.read_exact(&mut cbuffer) {
                Ok(_) => (),
                Err(e) => { debug!("seeddb read failed: {}", e); return new_key; }
            }
            let seed_count = LittleEndian::read_u32(&cbuffer);
            let _ = seeddb.seek(SeekFrom::Current(12));

            for _ in 0..seed_count {
                if seeddb.read_exact(&mut kbuffer).is_err() { break; }
                kbuffer.reverse();
                let key = hex::encode(kbuffer);
                if seeddb.read_exact(&mut sbuffer).is_err() { break; }
                seeds.insert(key, sbuffer);
                let _ = seeddb.seek(SeekFrom::Current(8));
            }
        }
        Err(_) => debug!("{} not found, trying to connect to Nintendo servers...", db_path.display())
    }

    // Check into Nintendo's servers
    if run.opts.allow_online && !seeds.contains_key(&titleid) {
        for country in ["JP", "US", "GB", "KR", "TW", "AU", "NZ"] {
            match attohttpc::get(format!("https://kagiya-ctr.cdn.nintendo.net/title/0x{}/ext_key?country={}", titleid, country))
                .danger_accept_invalid_certs(true)
                .send()
            {
                Ok(req) => {
                    if req.is_success() {
                        match req.bytes() {
                            Ok(bytes) => {
                                match <[u8; 16]>::try_from(bytes.as_ref()) {
                                    Ok(bytes) => {
                                        seeds.insert(titleid.clone(), bytes);
                                        debug!("A seed has been found online in the region {}", country);
                                        break;
                                    }
                                    Err(_) => ()
                                }
                            }
                            Err(_) => ()
                        }
                    }
                }
                Err(_) => {
                    // offline: don't panic upstream-style, just continue without the seed
                    debug!("online seed lookup failed; running without it");
                    break;
                }
            }
        }
    }

    if seeds.contains_key(&titleid) {
        let seed_check = BigEndian::read_u32(&header.seedcheck);
        let mut revtid = hex::decode(&titleid).unwrap();
        revtid.reverse();
        let sha_sum = sha256::digest([seeds[&titleid].to_vec(), revtid].concat());

        if BigEndian::read_u32(&hex::decode(sha_sum.get(0..8).unwrap()).unwrap()) == seed_check {
            let keystr = sha256::digest([u128::to_be_bytes(key_y), seeds[&titleid]].concat());
            new_key = BigEndian::read_u128(&hex::decode(keystr.get(0..32).unwrap()).unwrap());
        }
    }

    new_key
}

fn parse_ncsd(cia: &mut CiaReader, run: &mut Run) {
    debug!("Parsing NCSD in file: {}", cia.name);
    cia.seek(0);
    let mut tmp: [u8; 512] = [0u8; 512];
    cia.read(&mut tmp);
    run.report(512);
    let header: NcsdHdr = unsafe { std::mem::transmute(tmp) };
    for idx in 0..header.offset_sizetable.len() {
        if header.offset_sizetable[idx].offset != 0 {
            cia.cidx = idx as u16;
            cia.content_id = idx as u32;
            let mut tid: [u8; 8] = header.titleid;
            tid.reverse();
            parse_ncch(cia, (header.offset_sizetable[idx].offset * MEDIA_UNIT_SIZE) as u64, tid, run);
        }
    }
}

fn parse_ncch(cia: &mut CiaReader, offs: u64, mut titleid: [u8; 8], run: &mut Run) {
    if cia.from_ncsd {
        debug!("  Parsing {} NCCH", NCSD_PARTITIONS[cia.cidx as usize]);
    } else if cia.single_ncch {
        debug!("  Parsing NCCH in file: {}", cia.name);
    } else {
        debug!("Parsing NCCH: {}", cia.cidx)
    }

    cia.seek(offs);
    let mut tmp = [0u8; 512];
    cia.read(&mut tmp);
    run.report(512);
    let header: NcchHdr = unsafe { std::mem::transmute(tmp) };
    if titleid.iter().all(|&x| x == 0) {
        titleid = header.programid;
        titleid.reverse();
    }

    let ncch_key_y = BigEndian::read_u128(header.signature[0..16].try_into().unwrap());
    let mut tid: [u8; 8] = header.titleid;
    tid.reverse();

    debug!("  Product code: {}", std::str::from_utf8(&header.productcode).unwrap_or("?????????"));
    debug!("  KeyY: {:032X}", ncch_key_y);
    debug!("  Title ID: {}", hex::encode(tid).to_uppercase());
    debug!("  Content ID: {:08X}\n", cia.content_id);
    debug!("  Format version: {}\n", header.formatversion);

    let uses_extra_crypto: u8 = header.flags[3];

    if flag_to_bool(uses_extra_crypto) {
        debug!("  Uses extra NCCH crypto, keyslot 0x25");
    }

    let mut fixed_crypto: u8 = 0;
    let mut encrypted: bool = true;

    if flag_to_bool(header.flags[7] & 1) {
        if flag_to_bool(tid[3] & 16) { fixed_crypto = 2 } else { fixed_crypto = 1 }
        debug!("  Uses fixed-key crypto")
    }

    if flag_to_bool(header.flags[7] & 4) {
        encrypted = false;
        debug!("  Not encrypted")
    }

    let use_seed_crypto: bool = (header.flags[7] & 32) != 0;
    let mut key_y = ncch_key_y;

    if use_seed_crypto {
        key_y = get_new_key(ncch_key_y, &header, hex::encode(titleid), run);
        debug!("Uses 9.6 NCCH Seed crypto with KeyY: {:032X}", key_y);
    }

    let mut base: String;
    let path = Path::new(&cia.name);
    let file_name = path.file_name().map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // upstream stripped .3ds/.cia blindly (panic on .cci); tolerate all known roms
    base = file_name.trim_end_matches(".3ds").trim_end_matches(".cci").trim_end_matches(".cia").to_string();
    if base == file_name {
        // unknown extension: keep as-is (outputs get .ncch suffix anyway)
        base = file_name.to_string();
    }

    let final_path = canonicalize(&path).unwrap_or_else(|_| path.to_path_buf());
    let final_path = if cfg!(windows) && final_path.to_string_lossy().starts_with(r"\\?\") {
        Path::new(&final_path.to_string_lossy()[4..].replace("\\", "/")).to_path_buf()
    } else {
        final_path
    };
    let parent_dir = final_path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();

    let partname = if cia.from_ncsd { NCSD_PARTITIONS[cia.cidx as usize].to_string() } else { cia.cidx.to_string() };
    let out_path = parent_dir.join(format!("{}.{}.{:08X}.ncch", base, partname, cia.content_id));

    let mut ncch: File = match File::create(&out_path) {
        Ok(f) => f,
        Err(e) => { debug!("create {} failed", e); return; }
    };
    run.outputs.push(out_path.clone());
    tmp[399] = tmp[399] & 2 | 4;

    if ncch.write_all(&tmp).is_err() { return; }
    let mut counter: [u8; 16];
    if header.exhdrsize != 0 {
        counter = get_ncch_aes_counter(&header, NcchSection::ExHeader);
        dump_section(&mut ncch, cia, 512, header.exhdrsize * 2, NcchSection::ExHeader, 0, counter, uses_extra_crypto, fixed_crypto, use_seed_crypto, encrypted, [ncch_key_y, key_y], run);
    }

    if header.exefssize != 0 {
        counter = get_ncch_aes_counter(&header, NcchSection::ExeFS);
        dump_section(&mut ncch, cia, (header.exefsoffset * MEDIA_UNIT_SIZE) as u64, header.exefssize * MEDIA_UNIT_SIZE, NcchSection::ExeFS, 1, counter, uses_extra_crypto, fixed_crypto, use_seed_crypto, encrypted, [ncch_key_y, key_y], run);
    }

    if header.romfssize != 0 {
        counter = get_ncch_aes_counter(&header, NcchSection::RomFS);
        dump_section(&mut ncch, cia, (header.romfsoffset * MEDIA_UNIT_SIZE) as u64, header.romfssize * MEDIA_UNIT_SIZE, NcchSection::RomFS, 2, counter, uses_extra_crypto, fixed_crypto, use_seed_crypto, encrypted, [ncch_key_y, key_y], run);
    }

    info!("{}", out_path.display());
}

fn parse_cia(mut romfile: File, filename: String, run: &mut Run) {
    romfile.seek(SeekFrom::Start(0)).unwrap();
    let mut tmp: [u8; 32] = [0; 32];
    romfile.read_exact(&mut tmp).unwrap();
    let cia: CiaFile = unsafe { std::mem::transmute(tmp) };

    let cachainoff = align(cia.headersize as u64, 64);
    let tikoff = align(cachainoff + cia.cachainsize as u64, 64);
    let tmdoff = align(tikoff + cia.tiksize as u64, 64);
    let contentoffs = align(tmdoff + cia.tmdsize as u64, 64);

    romfile.seek(SeekFrom::Start((tikoff + 127 + 320) as u64)).unwrap();
    let mut enckey: [u8; 16] = [0; 16];
    romfile.read_exact(&mut enckey).unwrap();
    romfile.seek(SeekFrom::Start((tikoff + 156 + 320) as u64)).unwrap();
    let mut tid: [u8; 16] = [0; 16];
    romfile.read_exact(&mut tid[0..8]).unwrap();

    if hex::encode(tid).starts_with("00048") {
        debug!("Unsupported CIA file");
        return
    }

    romfile.seek(SeekFrom::Start((tikoff + 177 + 320) as u64)).unwrap();
    let mut cmnkeyidx: u8 = 0;
    romfile.read_exact(std::slice::from_mut(&mut cmnkeyidx)).unwrap();

    cbc_decrypt(&CMNKEYS[cmnkeyidx as usize], &tid, &mut enckey);
    let titkey = enckey;

    romfile.seek(SeekFrom::Start((tmdoff + 518) as u64)).unwrap();
    let mut content_count: [u8; 2] = [0; 2];
    romfile.read_exact(&mut content_count).unwrap();

    let mut next_content_offs = 0;
    for i in 0..BigEndian::read_u16(&content_count) {
        romfile.seek(SeekFrom::Start(tmdoff + 2820 + (48 * i as u64))).unwrap();
        let mut cbuffer: [u8; 16] = [0; 16];
        romfile.read_exact(&mut cbuffer).unwrap();

        let mut bcursor = Cursor::new(cbuffer);
        let content = CiaContent {
            cid:   bcursor.read_u32::<BigEndian>().unwrap(),
            cidx:  bcursor.read_u16::<BigEndian>().unwrap(),
            ctype: bcursor.read_u16::<BigEndian>().unwrap(),
            csize: bcursor.read_u64::<BigEndian>().unwrap()
        };

        let cenc: bool = (content.ctype & 1) != 0;

        romfile.seek(SeekFrom::Start((contentoffs + next_content_offs) as u64)).unwrap();
        let mut test: [u8; 512] = [0; 512];
        romfile.read_exact(&mut test).unwrap();
        let mut search: [u8; 4] = test[256..260].try_into().unwrap();

        let iv: [u8; 16] = gen_iv(content.cidx);

        if cenc {
            cbc_decrypt(&titkey, &iv, &mut test);
            search = test[256..260].try_into().unwrap();
        }

        match std::str::from_utf8(&search) {
            Ok(utf8) => if utf8 == "NCCH"
            {
                romfile.seek(SeekFrom::Start(contentoffs + next_content_offs)).unwrap();
                let mut cia_handle = CiaReader::new(romfile.try_clone().unwrap(), cenc, filename.clone(), titkey, content.cid, content.cidx, contentoffs + next_content_offs, false, false);
                next_content_offs += align(content.csize, 64);

                match run.opts.partition {
                    Some(number) => if (i as u8) != number { continue; },
                    None => (),
                }
                parse_ncch(&mut cia_handle, 0, tid[0..8].try_into().unwrap(), run);

            } else { debug!("CIA content can't be parsed, skipping partition") }
            Err(_) => debug!("CIA content can't be parsed, skipping partition")
        }
    }
}

/// Which ROM container a file is, from its 256..260 magic (NCSD/NCCH) or CIA header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RomKind {
    Ncsd,
    Ncch,
    Cia,
}

/// Lightweight "probe": what kind of rom is this? (used by the TUI before decrypting)
pub fn probe_kind(path: &Path) -> Option<RomKind> {
    let mut f = File::open(path).ok()?;
    f.seek(SeekFrom::Start(256)).ok()?;
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic).ok()?;
    match std::str::from_utf8(&magic) {
        Ok(ptype) if ptype == "NCSD" => { return Some(RomKind::Ncsd); }
        Ok(ptype) if ptype == "NCCH" => { return Some(RomKind::Ncch); }
        _ => ()
    }
    if path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("cia")).unwrap_or(false) {
        Some(RomKind::Cia)
    } else {
        None
    }
}

/// Decrypt a single rom file (calls the lib's parse pipeline with progress).
pub fn decrypt(path: &Path, opts: &DecryptOptions) -> Result<Vec<PathBuf>, DecryptError> {
    if !Path::exists(path) {
        return Err(DecryptError::NotSupported("path does not exist".to_string()));
    }

    let mut rom = File::open(path)?;
    let total = rom.metadata()?.len();
    let _ = rom.seek(SeekFrom::Start(256));
    let mut magic: [u8; 4] = [0u8; 4];
    rom.read_exact(&mut magic)?;

    let run = Run {
        done: 0,
        total,
        opts,
        outputs: vec![],
    };

    match std::str::from_utf8(&magic) {
        Ok(ptype) if ptype == "NCSD" => {
            let mut r = run;
            let mut reader = CiaReader::new(rom.try_clone().unwrap(), false, path.to_string_lossy().to_string(), [0u8; 16], 0, 0, 0, false, true);
            parse_ncsd(&mut reader, &mut r);
            Ok(r.outputs)
        }
        Ok(ptype) if ptype == "NCCH" => {
            let mut r = run;
            let mut reader = CiaReader::new(rom.try_clone().unwrap(), false, path.to_string_lossy().to_string(), [0u8; 16], 0, 0, 0, true, false);
            parse_ncch(&mut reader, 0, [0u8; 8], &mut r);
            Ok(r.outputs)
        }
        _ => {
            if !path.extension().map(|e| e.to_string_lossy().eq_ignore_ascii_case("cia")).unwrap_or(false) {
                return Err(DecryptError::NotSupported(format!("unknown file type (magic {})", String::from_utf8_lossy(&magic))));
            }
            let mut check: [u8; 4] = [0; 4];
            rom.read_exact(&mut check)?;
            if check[2..4] != [0, 0] {
                return Err(DecryptError::NotSupported(format!("bad CIA header (bytes {} {}), refusing to parse", check[2], check[3])));
            }
            let mut r = run;
            parse_cia(rom, path.to_string_lossy().to_string(), &mut r);
            Ok(r.outputs)
        }
    }
}

/// Run the upstream CLI behaviour (used by the binary).
pub fn run_cli(args: Vec<String>, verbose: bool) {
    if args.len() < 2 {
        println!("Usage: ctrdecrypt <ROMFILE> [OPTIONS]\nOptions:\n\t--ncch <partition-number>\n\t--no-verbose");
        return;
    }

    if !Path::exists(Path::new(&args[1])) {
        println!("ROM does not exist");
        return;
    }

    env_logger::Builder::new()
        .format(|buf, record| writeln!(buf, "{}", record.args()))
        .filter(None, if verbose { LevelFilter::Debug } else { LevelFilter::Info })
        .init();

    let opts = DecryptOptions {
        verbose,
        ..Default::default()
    };
    let path = PathBuf::from(&args[1]);
    match decrypt(&path, &opts) {
        Ok(outputs) => {
            for o in outputs {
                info!("{}", o.display());
            }
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}
