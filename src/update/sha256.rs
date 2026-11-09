//! SHA-256 (FIPS 180-4), enough to verify a downloaded update.

const BLOCK: usize = 64;
pub const DIGEST_LEN: usize = 32;

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const H0: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];

/// Digest of `data`, e.g. `digest(b"abc")` -> `ba7816bf…`.
pub fn digest(data: &[u8]) -> [u8; DIGEST_LEN] {
    let mut h = H0;

    // Message, 0x80, zeros, then the bit length as a big-endian u64.
    let bits = (data.len() as u64).wrapping_mul(8);
    let mut tail = Vec::with_capacity(2 * BLOCK);
    let full = data.len() - data.len() % BLOCK;
    tail.extend_from_slice(&data[full..]);
    tail.push(0x80);
    while tail.len() % BLOCK != BLOCK - 8 {
        tail.push(0);
    }
    tail.extend_from_slice(&bits.to_be_bytes());

    let (body, _) = data[..full].as_chunks::<BLOCK>();
    let (end, _) = tail.as_chunks::<BLOCK>();
    for block in body.iter().chain(end) {
        compress(&mut h, block);
    }

    let mut out = [0u8; DIGEST_LEN];
    for (o, w) in out.as_chunks_mut::<4>().0.iter_mut().zip(h) {
        *o = w.to_be_bytes();
    }
    out
}

fn compress(h: &mut [u32; 8], block: &[u8; BLOCK]) {
    let mut w = [0u32; 64];
    for (i, b) in block.as_chunks::<4>().0.iter().enumerate() {
        w[i] = u32::from_be_bytes(*b);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
    }

    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (x, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
        *x = x.wrapping_add(v);
    }
}

/// 64 hex digits (either case) -> digest; anything else is `None`.
pub fn parse_hex(s: &str) -> Option<[u8; DIGEST_LEN]> {
    let b = s.as_bytes();
    if b.len() != 2 * DIGEST_LEN {
        return None;
    }
    let nibble = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = [0u8; DIGEST_LEN];
    for (o, pair) in out.iter_mut().zip(b.as_chunks::<2>().0) {
        *o = nibble(pair[0])? << 4 | nibble(pair[1])?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(d: [u8; DIGEST_LEN]) -> String {
        d.iter().map(|b| format!("{b:02x}")).collect()
    }

    // NIST FIPS 180-4 examples (SHA256.pdf, SHA2_Additional.pdf).
    #[test]
    fn nist_vectors() {
        assert_eq!(hex(digest(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(hex(digest(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        let two_blocks = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(hex(digest(two_blocks)), "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
        let million = vec![b'a'; 1_000_000];
        assert_eq!(hex(digest(&million)), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
    }

    #[test]
    fn padding_edges() {
        // 55 bytes fit one block with the length; 56 and 64 need two.
        assert_eq!(hex(digest(&[0u8; 55])), "02779466cdec163811d078815c633f21901413081449002f24aa3e80f0b88ef7");
        assert_eq!(hex(digest(&[0u8; 56])), "d4817aa5497628e7c77e6b606107042bbba3130888c5f47a375e6179be789fbb");
        assert_eq!(hex(digest(&[0u8; 64])), "f5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a92759fb4b");
    }

    #[test]
    fn hex_parsing() {
        let h = "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD";
        assert_eq!(parse_hex(h), Some(digest(b"abc")));
        assert_eq!(parse_hex(&h[2..]), None);
        assert_eq!(parse_hex(&format!("{}zz", &h[2..])), None);
        assert_eq!(parse_hex(&format!("{}é", &h[1..63])), None);
    }
}
