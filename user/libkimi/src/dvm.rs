//! mini-Dalvik: a bounded, register-based DEX interpreter with a hand-written
//! micro core library. It EXECUTES purpose-built DEX (compiled from your own
//! Java via `javac` + `d8`, or hand-written smali) on KimiOS, printing to the
//! console. It is NOT an Android runtime and does not run Play-Store APKs —
//! see docs/android-feasibility.md (Stage 2 / layer L2).
//!
//! Frozen scope: single-threaded, no GC (bump arenas), no-op monitors, no
//! bytecode verifier (trusted DEX), a fixed opcode subset, and native shims for
//! Object / String / StringBuilder / System.out / PrintStream. Unimplemented
//! opcodes trap with an error rather than misbehaving.
//!
//! The module is self-contained (no syscalls, no crate:: imports) so the exact
//! same code runs as a host-side unit test and on the bare-metal guest.

// Object reference tagging: a register holds either a 32-bit primitive or a
// tagged object reference (REF_TAG | arena_index). Null is 0.
const REF_TAG: u32 = 0x8000_0000;

const MAX_OBJS: usize = 512;
const STRDATA_CAP: usize = 65536;
const ARRAYDATA_CAP: usize = 4096;
const OUT_CAP: usize = 8192;
const REG_CAP: usize = 4096;
const MAX_METHODS: usize = 1024;
const MAX_STATICS: usize = 256;

// Object kinds.
const K_FREE: u8 = 0;
const K_STRING: u8 = 1;
const K_BUILDER: u8 = 2;
const K_PRINTSTREAM: u8 = 3;
const K_ARRAY: u8 = 4;
const K_GENERIC: u8 = 5;

#[derive(Clone, Copy)]
struct Obj {
    kind: u8,
    len: u32,
    off: u32,
    fields: [u32; 8],
}

static mut OBJS: [Obj; MAX_OBJS] = [Obj { kind: K_FREE, len: 0, off: 0, fields: [0; 8] }; MAX_OBJS];
static mut OBJ_COUNT: usize = 0;
static mut STRDATA: [u8; STRDATA_CAP] = [0; STRDATA_CAP];
static mut STR_TOP: usize = 0;
static mut ARRAYDATA: [u32; ARRAYDATA_CAP] = [0; ARRAYDATA_CAP];
static mut ARRAY_TOP: usize = 0;
static mut OUT: [u8; OUT_CAP] = [0; OUT_CAP];
static mut OUT_LEN: usize = 0;
static mut REGS: [u32; REG_CAP] = [0; REG_CAP];
static mut REG_TOP: usize = 0;
static mut METHOD_CODE: [u32; MAX_METHODS] = [0; MAX_METHODS];
static mut STATIC_FIELDS: [u32; MAX_STATICS] = [0; MAX_STATICS];
static mut RESULT: u32 = 0;
static mut RESULT_HI: u32 = 0;
static mut ERR: &str = "";

fn objs_ptr() -> *mut Obj {
    core::ptr::addr_of_mut!(OBJS) as *mut Obj
}
fn strdata_ptr() -> *mut u8 {
    core::ptr::addr_of_mut!(STRDATA) as *mut u8
}
fn arraydata_ptr() -> *mut u32 {
    core::ptr::addr_of_mut!(ARRAYDATA) as *mut u32
}
fn out_ptr() -> *mut u8 {
    core::ptr::addr_of_mut!(OUT) as *mut u8
}
fn regs_ptr() -> *mut u32 {
    core::ptr::addr_of_mut!(REGS) as *mut u32
}

fn is_ref(v: u32) -> bool {
    v & REF_TAG != 0
}
fn ref_idx(v: u32) -> usize {
    (v & !REF_TAG) as usize
}
fn obj_of(v: u32) -> *mut Obj {
    unsafe { objs_ptr().add(ref_idx(v)) }
}

fn set_err(e: &'static str) {
    unsafe {
        *core::ptr::addr_of_mut!(ERR) = e;
    }
}

pub fn last_error() -> &'static str {
    unsafe { *core::ptr::addr_of!(ERR) }
}

/// Copy the program's accumulated console output into `dst`; returns the length.
pub fn take_output(dst: &mut [u8]) -> usize {
    unsafe {
        let n = (*core::ptr::addr_of!(OUT_LEN)).min(dst.len());
        core::ptr::copy_nonoverlapping(out_ptr(), dst.as_mut_ptr(), n);
        n
    }
}

fn out_byte(c: u8) {
    unsafe {
        let n = *core::ptr::addr_of!(OUT_LEN);
        if n < OUT_CAP {
            *out_ptr().add(n) = c;
            *core::ptr::addr_of_mut!(OUT_LEN) = n + 1;
        }
    }
}

fn out_bytes(s: &[u8]) {
    for &c in s {
        out_byte(c);
    }
}

fn out_int(mut v: i32) {
    if v < 0 {
        out_byte(b'-');
        v = -v;
    }
    let mut buf = [0u8; 12];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    out_bytes(&buf[i..]);
}

fn str_alloc(len: usize) -> Option<usize> {
    unsafe {
        let top = *core::ptr::addr_of!(STR_TOP);
        if top + len > STRDATA_CAP {
            return None;
        }
        *core::ptr::addr_of_mut!(STR_TOP) = top + len;
        Some(top)
    }
}

fn str_slice(off: u32, len: u32) -> &'static [u8] {
    unsafe { core::slice::from_raw_parts(strdata_ptr().add(off as usize), len as usize) }
}

fn new_obj(kind: u8) -> Option<u32> {
    unsafe {
        let n = *core::ptr::addr_of!(OBJ_COUNT);
        if n >= MAX_OBJS {
            return None;
        }
        *core::ptr::addr_of_mut!(OBJ_COUNT) = n + 1;
        let o = objs_ptr().add(n);
        (*o).kind = kind;
        (*o).len = 0;
        (*o).off = 0;
        (*o).fields = [0; 8];
        Some(REF_TAG | n as u32)
    }
}

fn make_string(bytes: &[u8]) -> Option<u32> {
    let off = str_alloc(bytes.len())?;
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), strdata_ptr().add(off), bytes.len());
    }
    let r = new_obj(K_STRING)?;
    unsafe {
        let o = obj_of(r);
        (*o).off = off as u32;
        (*o).len = bytes.len() as u32;
    }
    Some(r)
}

fn new_array(len: usize) -> Option<u32> {
    unsafe {
        let top = *core::ptr::addr_of!(ARRAY_TOP);
        if top + len > ARRAYDATA_CAP {
            return None;
        }
        *core::ptr::addr_of_mut!(ARRAY_TOP) = top + len;
        let r = new_obj(K_ARRAY)?;
        let o = obj_of(r);
        (*o).off = top as u32;
        (*o).len = len as u32;
        Some(r)
    }
}

// ---------------------------------------------------------------------------
// DEX parsing.
// ---------------------------------------------------------------------------

fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn uleb(b: &[u8], o: &mut usize) -> u32 {
    let mut r = 0u32;
    let mut s = 0;
    loop {
        let x = b[*o];
        *o += 1;
        r |= ((x & 0x7f) as u32) << s;
        if x & 0x80 == 0 {
            break;
        }
        s += 7;
    }
    r
}

pub struct Vm<'a> {
    b: &'a [u8],
    string_ids_off: usize,
    string_ids_size: usize,
    type_ids_off: usize,
    proto_ids_off: usize,
    field_ids_off: usize,
    method_ids_off: usize,
    method_ids_size: usize,
    class_defs_off: usize,
    class_defs_size: usize,
}

impl<'a> Vm<'a> {
    fn new(b: &'a [u8]) -> Self {
        Vm {
            b,
            string_ids_off: 0,
            string_ids_size: 0,
            type_ids_off: 0,
            proto_ids_off: 0,
            field_ids_off: 0,
            method_ids_off: 0,
            method_ids_size: 0,
            class_defs_off: 0,
            class_defs_size: 0,
        }
    }

    fn load(&mut self) -> bool {
        let b = self.b;
        if b.len() < 112 || &b[..4] != b"dex\n" {
            set_err("not a DEX file");
            return false;
        }
        self.string_ids_size = le32(b, 56) as usize;
        self.string_ids_off = le32(b, 60) as usize;
        self.type_ids_off = le32(b, 68) as usize;
        self.proto_ids_off = le32(b, 76) as usize;
        self.field_ids_off = le32(b, 84) as usize;
        self.method_ids_size = le32(b, 88) as usize;
        self.method_ids_off = le32(b, 92) as usize;
        self.class_defs_size = le32(b, 96) as usize;
        self.class_defs_off = le32(b, 100) as usize;

        unsafe {
            let mc = core::ptr::addr_of_mut!(METHOD_CODE) as *mut u32;
            for i in 0..MAX_METHODS {
                *mc.add(i) = 0;
            }
        }
        // Map every method_id to its code_off (0 = native / abstract / no code).
        for ci in 0..self.class_defs_size {
            let cd = self.class_defs_off + ci * 32;
            if cd + 32 > b.len() {
                break;
            }
            let class_data = le32(b, cd + 24) as usize;
            if class_data == 0 {
                continue;
            }
            let mut o = class_data;
            let sf = uleb(b, &mut o) as usize;
            let inf = uleb(b, &mut o) as usize;
            let dm = uleb(b, &mut o) as usize;
            let vm = uleb(b, &mut o) as usize;
            for _ in 0..(sf + inf) {
                uleb(b, &mut o);
                uleb(b, &mut o);
            }
            for count in [dm, vm] {
                // The first method of each list (direct, then virtual) carries an
                // absolute index; subsequent entries are diffs within that list.
                let mut midx = 0u32;
                for _ in 0..count {
                    midx += uleb(b, &mut o);
                    uleb(b, &mut o); // access flags
                    let code = uleb(b, &mut o);
                    if (midx as usize) < MAX_METHODS {
                        unsafe {
                            *(core::ptr::addr_of_mut!(METHOD_CODE) as *mut u32)
                                .add(midx as usize) = code;
                        }
                    }
                }
            }
        }
        true
    }

    fn string(&self, idx: u32) -> &'a [u8] {
        if idx as usize >= self.string_ids_size {
            return &[];
        }
        let off = le32(self.b, self.string_ids_off + idx as usize * 4) as usize;
        if off >= self.b.len() {
            return &[];
        }
        let mut o = off;
        uleb(self.b, &mut o); // utf16 length
        let start = o;
        while o < self.b.len() && self.b[o] != 0 {
            o += 1;
        }
        &self.b[start..o]
    }

    fn type_desc(&self, idx: u32) -> &'a [u8] {
        let off = self.type_ids_off + idx as usize * 4;
        if off + 4 > self.b.len() {
            return &[];
        }
        self.string(le32(self.b, off))
    }

    fn method_parts(&self, midx: u32) -> (u16, u16, u32) {
        let off = self.method_ids_off + midx as usize * 8;
        (le16(self.b, off), le16(self.b, off + 2), le32(self.b, off + 4))
    }
    fn method_class(&self, midx: u32) -> &'a [u8] {
        self.type_desc(self.method_parts(midx).0 as u32)
    }
    fn method_name(&self, midx: u32) -> &'a [u8] {
        self.string(self.method_parts(midx).2)
    }
    fn field_parts(&self, fidx: u32) -> (u16, u16, u32) {
        let off = self.field_ids_off + fidx as usize * 8;
        (le16(self.b, off), le16(self.b, off + 2), le32(self.b, off + 4))
    }

    fn param_type(&self, midx: u32, n: usize) -> &'a [u8] {
        let proto_idx = self.method_parts(midx).1 as usize;
        let po = self.proto_ids_off + proto_idx * 12;
        if po + 12 > self.b.len() {
            return &[];
        }
        let params_off = le32(self.b, po + 8) as usize;
        if params_off == 0 || params_off + 4 > self.b.len() {
            return &[];
        }
        let size = le32(self.b, params_off) as usize;
        if n >= size {
            return &[];
        }
        let to = params_off + 4 + n * 2;
        if to + 2 > self.b.len() {
            return &[];
        }
        self.type_desc(le16(self.b, to) as u32)
    }

    fn find_main(&self) -> Option<u32> {
        for midx in 0..self.method_ids_size as u32 {
            if self.method_name(midx) == b"main" {
                let code = unsafe {
                    *(core::ptr::addr_of!(METHOD_CODE) as *const u32).add(midx as usize)
                };
                if code != 0 {
                    return Some(midx);
                }
            }
        }
        None
    }

    fn u16(&self, o: usize) -> u16 {
        le16(self.b, o)
    }
    fn u32(&self, o: usize) -> u32 {
        le32(self.b, o)
    }

    // -- object / library helpers ------------------------------------------

    fn new_instance(&mut self, desc: &[u8]) -> Option<u32> {
        if desc == b"Ljava/lang/StringBuilder;" || desc == b"Ljava/lang/StringBuffer;" {
            let r = new_obj(K_BUILDER)?;
            unsafe {
                let o = obj_of(r);
                (*o).off = *core::ptr::addr_of!(STR_TOP) as u32;
                (*o).len = 0;
            }
            Some(r)
        } else {
            new_obj(K_GENERIC)
        }
    }

    fn obj_string_bytes(&self, v: u32) -> &'static [u8] {
        if !is_ref(v) {
            return &[];
        }
        unsafe {
            let o = obj_of(v);
            match (*o).kind {
                K_STRING | K_BUILDER => str_slice((*o).off, (*o).len),
                _ => &[],
            }
        }
    }

    fn print_value(&mut self, v: u32, ptype: &[u8]) {
        if ptype == b"I" || ptype == b"S" || ptype == b"B" {
            out_int(v as i32);
        } else if ptype == b"J" {
            out_int(v as i32);
        } else if ptype == b"C" {
            out_byte(v as u8);
        } else if ptype == b"Z" {
            out_bytes(if v != 0 { b"true" } else { b"false" });
        } else if is_ref(v) {
            let bytes = self.obj_string_bytes(v);
            if bytes.is_empty() {
                unsafe {
                    let o = obj_of(v);
                    if (*o).kind == K_ARRAY {
                        out_bytes(b"[array]");
                    } else {
                        out_bytes(b"object");
                    }
                }
            } else {
                out_bytes(bytes);
            }
        } else {
            out_int(v as i32);
        }
    }

    fn sget(&mut self, fidx: u32) -> u32 {
        let (class_idx, _t, name_idx) = self.field_parts(fidx);
        let class = self.type_desc(class_idx as u32);
        let name = self.string(name_idx);
        if class == b"Ljava/lang/System;" && name == b"out" {
            return unsafe { *core::ptr::addr_of!(PRINTSTREAM) };
        }
        if (fidx as usize) < MAX_STATICS {
            unsafe { *(core::ptr::addr_of!(STATIC_FIELDS) as *const u32).add(fidx as usize) }
        } else {
            0
        }
    }

    fn sput(&mut self, fidx: u32, v: u32) {
        if (fidx as usize) < MAX_STATICS {
            unsafe {
                *(core::ptr::addr_of_mut!(STATIC_FIELDS) as *mut u32).add(fidx as usize) = v;
            }
        }
    }

    fn append(&mut self, this: u32, v: u32, ptype: &[u8]) -> u32 {
        unsafe {
            let o = obj_of(this);
            let old_off = (*o).off as usize;
            let old_len = (*o).len as usize;
            // Materialize the appended bytes.
            let mut tmp = [0u8; 32];
            let (src_ptr, src_len): (*const u8, usize) = if ptype == b"I" || !is_ref(v) {
                let mut n = 0usize;
                let mut x = v as i32;
                if x < 0 {
                    tmp[n] = b'-';
                    n += 1;
                    x = -x;
                }
                let mut digits = [0u8; 12];
                let mut di = digits.len();
                loop {
                    di -= 1;
                    digits[di] = b'0' + (x % 10) as u8;
                    x /= 10;
                    if x == 0 {
                        break;
                    }
                }
                let dl = digits.len() - di;
                tmp[n..n + dl].copy_from_slice(&digits[di..]);
                (tmp.as_ptr(), n + dl)
            } else {
                let so = obj_of(v);
                (strdata_ptr().add((*so).off as usize), (*so).len as usize)
            };
            let new_len = old_len + src_len;
            let new_off = match str_alloc(new_len) {
                Some(x) => x,
                None => return this,
            };
            let dp = strdata_ptr();
            core::ptr::copy_nonoverlapping(dp.add(old_off), dp.add(new_off), old_len);
            core::ptr::copy_nonoverlapping(src_ptr, dp.add(new_off + old_len), src_len);
            (*o).off = new_off as u32;
            (*o).len = new_len as u32;
        }
        this
    }

    fn native(&mut self, midx: u32, args: &[u32]) -> u32 {
        let class = self.method_class(midx);
        let name = self.method_name(midx);
        if class == b"Ljava/lang/Object;" && name == b"<init>" {
            return 0;
        }
        if class == b"Ljava/io/PrintStream;" {
            let ptype = self.param_type(midx, 0);
            if name == b"println" {
                if !ptype.is_empty() && args.len() >= 2 {
                    self.print_value(args[1], ptype);
                }
                out_byte(b'\n');
                return 0;
            }
            if name == b"print" {
                if !ptype.is_empty() && args.len() >= 2 {
                    self.print_value(args[1], ptype);
                }
                return 0;
            }
        }
        if class == b"Ljava/lang/StringBuilder;" || class == b"Ljava/lang/StringBuffer;" {
            if name == b"<init>" {
                return args.first().copied().unwrap_or(0);
            }
            if name == b"append" && args.len() >= 2 {
                let ptype = self.param_type(midx, 0);
                return self.append(args[0], args[1], ptype);
            }
            if name == b"toString" {
                let r = new_obj(K_STRING).unwrap_or(0);
                if r != 0 {
                    unsafe {
                        let src = obj_of(args[0]);
                        let dst = obj_of(r);
                        (*dst).off = (*src).off;
                        (*dst).len = (*src).len;
                    }
                }
                return r;
            }
            if name == b"length" {
                return unsafe { (*obj_of(args[0])).len };
            }
        }
        if class == b"Ljava/lang/String;" {
            if name == b"length" {
                return unsafe { (*obj_of(args[0])).len };
            }
            if name == b"charAt" && args.len() >= 2 {
                let bytes = self.obj_string_bytes(args[0]);
                let i = args[1] as usize;
                return if i < bytes.len() { bytes[i] as u32 } else { 0 };
            }
        }
        set_err("unimplemented native method");
        0
    }

    fn invoke(&mut self, midx: u32, args: &[u32]) -> u32 {
        let code = unsafe {
            *(core::ptr::addr_of!(METHOD_CODE) as *const u32).add(midx as usize)
        };
        if code == 0 {
            return self.native(midx, args);
        }
        self.exec_code(code as usize, args)
    }

    fn exec_code(&mut self, code: usize, args: &[u32]) -> u32 {
        let b = self.b;
        let nregs = le16(b, code) as usize;
        let ins = le16(b, code + 2) as usize;
        let insns_size = le32(b, code + 12) as usize;
        let insns = code + 16;
        let base = unsafe { *core::ptr::addr_of!(REG_TOP) };
        if base + nregs > REG_CAP {
            set_err("register overflow");
            return 0;
        }
        unsafe {
            *core::ptr::addr_of_mut!(REG_TOP) = base + nregs;
        }
        let regs = unsafe { core::slice::from_raw_parts_mut(regs_ptr().add(base), nregs) };
        for r in regs.iter_mut() {
            *r = 0;
        }
        let argbase = nregs.saturating_sub(ins);
        for (i, &a) in args.iter().enumerate() {
            if argbase + i < nregs {
                regs[argbase + i] = a;
            }
        }

        let mut pc = 0usize;
        let ret = loop {
            if pc >= insns_size {
                break 0;
            }
            let u = self.u16(insns + pc * 2);
            let op = (u & 0xff) as u8;
            let a = ((u >> 8) & 0xf) as usize;
            let bb = ((u >> 12) & 0xf) as usize;
            let mut next = pc + 1;
            match op {
                0x00 => {} // nop
                0x01 => regs[a] = regs[bb],
                0x02 => {
                    let s = self.u16(insns + (pc + 1) * 2) as usize;
                    regs[a] = regs[s];
                    next = pc + 2;
                }
                0x04 | 0x05 => {
                    let s = if op == 0x04 {
                        bb
                    } else {
                        next = pc + 2;
                        self.u16(insns + (pc + 1) * 2) as usize
                    };
                    regs[a] = regs[s];
                    regs[a + 1] = regs[s + 1];
                }
                0x07 => regs[a] = regs[bb],
                0x08 => {
                    let s = self.u16(insns + (pc + 1) * 2) as usize;
                    regs[a] = regs[s];
                    next = pc + 2;
                }
                0x0a | 0x0c => regs[a] = unsafe { *core::ptr::addr_of!(RESULT) },
                0x0b => {
                    regs[a] = unsafe { *core::ptr::addr_of!(RESULT) };
                    regs[a + 1] = unsafe { *core::ptr::addr_of!(RESULT_HI) };
                }
                0x0d => regs[a] = 0,
                0x0e => break 0,
                0x0f | 0x11 => break regs[a],
                0x10 => {
                    unsafe {
                        *core::ptr::addr_of_mut!(RESULT_HI) = regs[a + 1];
                    }
                    break regs[a];
                }
                0x12 => {
                    let lit = if bb >= 8 { bb as i32 - 16 } else { bb as i32 };
                    regs[a] = lit as u32;
                }
                0x13 => {
                    let lit = self.u16(insns + (pc + 1) * 2) as i16 as i32;
                    regs[a] = lit as u32;
                    next = pc + 2;
                }
                0x14 => {
                    let lit = self.u32(insns + (pc + 1) * 2) as i32;
                    regs[a] = lit as u32;
                    next = pc + 3;
                }
                0x15 => {
                    let lit = (self.u16(insns + (pc + 1) * 2) as i32) << 16;
                    regs[a] = lit as u32;
                    next = pc + 2;
                }
                0x16 => {
                    let lit = self.u16(insns + (pc + 1) * 2) as i16 as i64;
                    regs[a] = lit as u32;
                    regs[a + 1] = (lit >> 32) as u32;
                    next = pc + 2;
                }
                0x1a => {
                    let sidx = self.u16(insns + (pc + 1) * 2) as u32;
                    let bytes = self.string(sidx);
                    regs[a] = make_string(bytes).unwrap_or(0);
                    next = pc + 2;
                }
                0x1b => {
                    let sidx = self.u32(insns + (pc + 1) * 2);
                    let bytes = self.string(sidx);
                    regs[a] = make_string(bytes).unwrap_or(0);
                    next = pc + 3;
                }
                0x1c => {
                    regs[a] = new_obj(K_GENERIC).unwrap_or(0);
                    next = pc + 2;
                }
                0x1d | 0x1e => {} // monitor-enter/exit: no-op (single-threaded)
                0x1f => next = pc + 2, // check-cast: trusted
                0x20 => {
                    regs[a] = if is_ref(regs[bb]) { 1 } else { 0 };
                    next = pc + 2;
                }
                0x21 => {
                    let v = regs[bb];
                    regs[a] = if is_ref(v) { unsafe { (*obj_of(v)).len } } else { 0 };
                }
                0x22 => {
                    let tidx = self.u16(insns + (pc + 1) * 2) as u32;
                    let desc = self.type_desc(tidx);
                    regs[a] = self.new_instance(desc).unwrap_or(0);
                    next = pc + 2;
                }
                0x23 => {
                    let len = regs[bb] as usize;
                    regs[a] = new_array(len).unwrap_or(0);
                    next = pc + 2;
                }
                0x28 => {
                    let off = (u >> 8) as i8 as i32;
                    next = (pc as i32 + off) as usize;
                }
                0x29 => {
                    let off = self.u16(insns + (pc + 1) * 2) as i16 as i32;
                    next = (pc as i32 + off) as usize;
                }
                0x2a => {
                    let off = self.u32(insns + (pc + 1) * 2) as i32;
                    next = (pc as i32 + off) as usize;
                }
                0x32..=0x37 => {
                    let off = self.u16(insns + (pc + 1) * 2) as i16 as i32;
                    let x = regs[a] as i32;
                    let y = regs[bb] as i32;
                    let take = match op {
                        0x32 => x == y,
                        0x33 => x != y,
                        0x34 => x < y,
                        0x35 => x >= y,
                        0x36 => x > y,
                        _ => x <= y,
                    };
                    next = if take { (pc as i32 + off) as usize } else { pc + 2 };
                }
                0x38..=0x3d => {
                    let off = self.u16(insns + (pc + 1) * 2) as i16 as i32;
                    let x = regs[a] as i32;
                    let take = match op {
                        0x38 => x == 0,
                        0x39 => x != 0,
                        0x3a => x < 0,
                        0x3b => x >= 0,
                        0x3c => x > 0,
                        _ => x <= 0,
                    };
                    next = if take { (pc as i32 + off) as usize } else { pc + 2 };
                }
                0x44..=0x4a => {
                    // aget variants: vA = vB[vC]
                    let cw = self.u16(insns + (pc + 1) * 2);
                    let objreg = (cw & 0xff) as usize;
                    let idxreg = (cw >> 8) as usize;
                    let arr = regs[objreg];
                    let idx = regs[idxreg] as usize;
                    regs[a] = if is_ref(arr) {
                        unsafe {
                            let o = obj_of(arr);
                            let base = (*o).off as usize;
                            if idx < (*o).len as usize {
                                *arraydata_ptr().add(base + idx)
                            } else {
                                0
                            }
                        }
                    } else {
                        0
                    };
                    next = pc + 2;
                }
                0x4b..=0x51 => {
                    // aput variants: vB[vC] = vA
                    let cw = self.u16(insns + (pc + 1) * 2);
                    let objreg = (cw & 0xff) as usize;
                    let idxreg = (cw >> 8) as usize;
                    let arr = regs[objreg];
                    let idx = regs[idxreg] as usize;
                    if is_ref(arr) {
                        unsafe {
                            let o = obj_of(arr);
                            let base = (*o).off as usize;
                            if idx < (*o).len as usize {
                                *arraydata_ptr().add(base + idx) = regs[a];
                            }
                        }
                    }
                    next = pc + 2;
                }
                0x52..=0x58 => {
                    // iget variants
                    let fidx = self.u16(insns + (pc + 1) * 2) as u32;
                    let ov = regs[bb];
                    regs[a] = if is_ref(ov) {
                        unsafe { (*obj_of(ov)).fields[(fidx as usize) % 8] }
                    } else {
                        0
                    };
                    next = pc + 2;
                }
                0x59..=0x5f => {
                    // iput variants
                    let fidx = self.u16(insns + (pc + 1) * 2) as u32;
                    let ov = regs[bb];
                    if is_ref(ov) {
                        unsafe {
                            (*obj_of(ov)).fields[(fidx as usize) % 8] = regs[a];
                        }
                    }
                    next = pc + 2;
                }
                0x60..=0x66 => {
                    let fidx = self.u16(insns + (pc + 1) * 2) as u32;
                    regs[a] = self.sget(fidx);
                    next = pc + 2;
                }
                0x67..=0x6d => {
                    let fidx = self.u16(insns + (pc + 1) * 2) as u32;
                    self.sput(fidx, regs[a]);
                    next = pc + 2;
                }
                0x6e..=0x72 => {
                    // invoke-kind {vC..}, meth@BBBB  (format 35c)
                    let count = bb;
                    let midx = self.u16(insns + (pc + 1) * 2) as u32;
                    let argw = self.u16(insns + (pc + 2) * 2);
                    let mut argregs = [0u32; 5];
                    argregs[0] = regs[(argw & 0xf) as usize];
                    argregs[1] = regs[((argw >> 4) & 0xf) as usize];
                    argregs[2] = regs[((argw >> 8) & 0xf) as usize];
                    argregs[3] = regs[((argw >> 12) & 0xf) as usize];
                    argregs[4] = regs[a];
                    let r = self.invoke(midx, &argregs[..count]);
                    unsafe {
                        *core::ptr::addr_of_mut!(RESULT) = r;
                    }
                    next = pc + 3;
                }
                0x74..=0x78 => {
                    // invoke-kind/range {vCCCC ..}, meth@BBBB  (format 3rc)
                    let count = ((u >> 8) & 0xff) as usize;
                    let midx = self.u16(insns + (pc + 1) * 2) as u32;
                    let first = self.u16(insns + (pc + 2) * 2) as usize;
                    let mut argregs = [0u32; 16];
                    let n = count.min(16);
                    for i in 0..n {
                        argregs[i] = regs[first + i];
                    }
                    let r = self.invoke(midx, &argregs[..n]);
                    unsafe {
                        *core::ptr::addr_of_mut!(RESULT) = r;
                    }
                    next = pc + 3;
                }
                0x7b => regs[a] = (-(regs[bb] as i32)) as u32,
                0x7d => regs[a] = !regs[bb],
                0x80 => regs[a] = (-(regs[bb] as i64)) as u32,
                0x90..=0x9a => {
                    let cw = self.u16(insns + (pc + 1) * 2);
                    let x = regs[(cw & 0xff) as usize] as i32;
                    let y = regs[(cw >> 8) as usize] as i32;
                    regs[a] = int_op(op, x, y) as u32;
                    next = pc + 2;
                }
                0xb0..=0xba => {
                    let x = regs[a] as i32;
                    let y = regs[bb] as i32;
                    regs[a] = int_op(op - 0x20, x, y) as u32;
                }
                0xd0..=0xd7 => {
                    let lit = self.u16(insns + (pc + 1) * 2) as i16 as i32;
                    let x = regs[bb] as i32;
                    regs[a] = int_op(op, x, lit) as u32;
                    next = pc + 2;
                }
                0xd8..=0xe2 => {
                    let cw = self.u16(insns + (pc + 1) * 2);
                    let x = regs[(cw & 0xff) as usize] as i32;
                    let lit = (cw >> 8) as i8 as i32;
                    regs[a] = int_op(op, x, lit) as u32;
                    next = pc + 2;
                }
                0xe3..=0xf9 => {
                    set_err("unsupported opcode (long/float/cast)");
                    break 0;
                }
                _ => {
                    set_err("unknown opcode");
                    break 0;
                }
            }
            pc = next;
        };
        unsafe {
            *core::ptr::addr_of_mut!(REG_TOP) = base;
        }
        ret
    }
}

// Semantic integer ops shared by the 3-register (0x90..), /2addr (0xb0..),
// /lit16 (0xd0..) and /lit8 (0xd8..) families. 0xd1/0xd9 are rsub (lit - x).
fn int_op(op: u8, x: i32, y: i32) -> i32 {
    match op {
        0x90 | 0xd0 | 0xd8 => x.wrapping_add(y),
        0x91 => x.wrapping_sub(y),
        0xd1 | 0xd9 => y.wrapping_sub(x),
        0x92 | 0xd2 | 0xda => x.wrapping_mul(y),
        0x93 | 0xd3 | 0xdb => {
            if y != 0 {
                x.wrapping_div(y)
            } else {
                0
            }
        }
        0x94 | 0xd4 | 0xdc => {
            if y != 0 {
                x.wrapping_rem(y)
            } else {
                0
            }
        }
        0x95 | 0xd5 | 0xdd => x & y,
        0x96 | 0xd6 | 0xde => x | y,
        0x97 | 0xd7 | 0xdf => x ^ y,
        0x98 | 0xe0 => x.wrapping_shl((y & 31) as u32),
        0x99 | 0xe1 => x.wrapping_shr((y & 31) as u32),
        0x9a | 0xe2 => ((x as u32).wrapping_shr((y & 31) as u32)) as i32,
        _ => 0,
    }
}

static mut PRINTSTREAM: u32 = 0;

fn reset() {
    unsafe {
        *core::ptr::addr_of_mut!(OBJ_COUNT) = 0;
        *core::ptr::addr_of_mut!(STR_TOP) = 0;
        *core::ptr::addr_of_mut!(ARRAY_TOP) = 0;
        *core::ptr::addr_of_mut!(OUT_LEN) = 0;
        *core::ptr::addr_of_mut!(REG_TOP) = 0;
        *core::ptr::addr_of_mut!(RESULT) = 0;
        *core::ptr::addr_of_mut!(RESULT_HI) = 0;
        *core::ptr::addr_of_mut!(ERR) = "";
        let sf = core::ptr::addr_of_mut!(STATIC_FIELDS) as *mut u32;
        for i in 0..MAX_STATICS {
            *sf.add(i) = 0;
        }
    }
}

/// Execute the `main` method of a purpose-built DEX image. Console output is
/// accumulated and retrieved with `take_output`. Returns false on load/run
/// error (see `last_error`).
pub fn run(dex: &[u8]) -> bool {
    reset();
    let mut vm = Vm::new(dex);
    if !vm.load() {
        return false;
    }
    unsafe {
        *core::ptr::addr_of_mut!(PRINTSTREAM) = new_obj(K_PRINTSTREAM).unwrap_or(0);
    }
    let Some(main) = vm.find_main() else {
        set_err("no main method");
        return false;
    };
    let args = new_array(0).unwrap_or(0);
    vm.invoke(main, &[args]);
    last_error().is_empty()
}