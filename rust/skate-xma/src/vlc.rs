// Port of FFmpeg 4.4 libavcodec/bitstream.c VLC table construction
// (ff_init_vlc_sparse / build_table, big-endian codes, symbols = code index).
// LGPL-2.1-or-later, see lib.rs.

#[derive(Clone, Copy)]
struct VlcCode {
    bits: u8,
    symbol: i16,
    /// codeword, with the first bit-to-be-read in the msb
    code: u32,
}

/// alloc_table: append `size` entries, return the first index.
fn alloc_table(table: &mut Vec<[i16; 2]>, size: usize) -> usize {
    let index = table.len();
    table.resize(index + size, [0, 0]);
    index
}

fn build_table(table: &mut Vec<[i16; 2]>, table_nb_bits: u32, codes: &mut [VlcCode]) -> usize {
    let table_size = 1usize << table_nb_bits;
    let table_index = alloc_table(table, table_size);
    let nb_codes = codes.len();
    let mut i = 0usize;
    while i < nb_codes {
        let n = codes[i].bits as u32;
        let code = codes[i].code;
        let symbol = codes[i].symbol;
        if n <= table_nb_bits {
            let nb = 1usize << (table_nb_bits - n);
            for j in ((code >> (32 - table_nb_bits)) as usize..).take(nb) {
                let e = &mut table[table_index + j];
                let bits = e[1] as i32;
                let oldsym = e[0];
                assert!(
                    !((bits != 0 || oldsym != 0) && (bits != n as i32 || oldsym != symbol)),
                    "incorrect codes"
                );
                e[1] = n as i16;
                e[0] = symbol;
            }
        } else {
            let n = n - table_nb_bits;
            let code_prefix = code >> (32 - table_nb_bits);
            let mut subtable_bits = n;
            codes[i].bits = n as u8;
            codes[i].code = code << table_nb_bits;
            let mut k = i + 1;
            while k < nb_codes {
                let n = codes[k].bits as i32 - table_nb_bits as i32;
                if n <= 0 {
                    break;
                }
                let code = codes[k].code;
                if code >> (32 - table_nb_bits) != code_prefix {
                    break;
                }
                codes[k].bits = n as u8;
                codes[k].code = code << table_nb_bits;
                subtable_bits = subtable_bits.max(n as u32);
                k += 1;
            }
            subtable_bits = subtable_bits.min(table_nb_bits);
            let j = code_prefix as usize;
            table[table_index + j][1] = -(subtable_bits as i16);
            let index = build_table(table, subtable_bits, &mut codes[i..k]);
            table[table_index + j][0] = index as i16;
            assert_eq!(table[table_index + j][0] as usize, index, "strange codes");
            i = k - 1;
        }
        i += 1;
    }
    for e in &mut table[table_index..table_index + table_size] {
        if e[1] == 0 {
            e[0] = -1;
        }
    }
    table_index
}

/// ff_init_vlc_sparse(vlc, nb_bits, nb_codes, bits, 1, 1, codes, size, size,
/// NULL, 0, 0, INIT_VLC_USE_NEW_STATIC) as used by INIT_VLC_STATIC.
pub fn init_vlc(nb_bits: u32, bits: &[u8], codes: &[u32]) -> Vec<[i16; 2]> {
    assert_eq!(bits.len(), codes.len());
    let mut buf: Vec<VlcCode> = Vec::with_capacity(bits.len());
    let copy = |cond: &dyn Fn(u32) -> bool, buf: &mut Vec<VlcCode>| {
        for (i, (&len, &code)) in bits.iter().zip(codes).enumerate() {
            let len = len as u32;
            if !cond(len) {
                continue;
            }
            assert!(len <= 3 * nb_bits && len <= 32, "Too long VLC");
            assert!((code as u64) < (1u64 << len), "Invalid code");
            buf.push(VlcCode { bits: len as u8, symbol: i as i16, code: code << (32 - len) });
        }
    };
    copy(&|len| len > nb_bits, &mut buf);
    // AV_QSORT by (code >> 1); codes are distinct so the order is total.
    buf.sort_by_key(|c| c.code >> 1);
    copy(&|len| len != 0 && len <= nb_bits, &mut buf);
    let mut table = Vec::new();
    build_table(&mut table, nb_bits, &mut buf);
    table
}
