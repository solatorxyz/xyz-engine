// The world the native fuzz targets run in: the program's own processor code, compiled for the host, called on accounts
// that live in this process. Shared by the targets through `include!` (each target is its own crate root).
//
// An account is a RuntimeAccount header followed by its data, exactly as the loader lays it out, so the program's
// zero-copy `load` reads it the way it reads a real account. `World::call` is one transaction with one instruction: on
// failure every account is restored, and an instruction that "succeeds" after writing to an account it was given
// read-only counts as failed, because the runtime would refuse it.
const TOKEN: Address = Address::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
const PYTH: Address = Address::from_str_const("rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ");
type K = [u8; 32];
type Meta = (K, bool, bool);
fn ad(k: &K) -> Address {
    Address::new_from_array(*k)
}
fn kk(a: &Address) -> K {
    let mut b = [0u8; 32];
    b.copy_from_slice(a.as_ref());
    b
}
const HDR: usize = core::mem::size_of::<RuntimeAccount>();
fn dbg() -> bool {
    static D: OnceLock<bool> = OnceLock::new();
    *D.get_or_init(|| std::env::var("NATIVE_DEBUG").is_ok())
}
struct Acct {
    buf: Vec<u64>,
} // 8-aligned: RuntimeAccount header + data
impl Acct {
    fn new(key: &Address, owner: &Address, data: &[u8]) -> Self {
        let mut buf = vec![0u64; (HDR + data.len() + 7) / 8 + 1];
        let p = buf.as_mut_ptr() as *mut u8;
        unsafe {
            let ra = p as *mut RuntimeAccount;
            (*ra).borrow_state = u8::MAX;
            (*ra).address = key.clone();
            (*ra).owner = owner.clone();
            (*ra).lamports = 1_000_000_000;
            (*ra).data_len = data.len() as u64;
            std::ptr::copy_nonoverlapping(data.as_ptr(), p.add(HDR), data.len());
        }
        Acct { buf }
    }
    fn ra(&mut self) -> *mut RuntimeAccount {
        self.buf.as_mut_ptr() as *mut RuntimeAccount
    }
    fn data(&self) -> &[u8] {
        unsafe {
            let p = self.buf.as_ptr() as *const u8;
            std::slice::from_raw_parts(p.add(HDR), (*(p as *const RuntimeAccount)).data_len as usize)
        }
    }
    fn data_mut(&mut self) -> &mut [u8] {
        unsafe {
            let p = self.buf.as_mut_ptr() as *mut u8;
            let n = (*(p as *const RuntimeAccount)).data_len as usize;
            std::slice::from_raw_parts_mut(p.add(HDR), n)
        }
    }
}
struct World {
    pid: Address,
    accts: HashMap<K, Acct>,
    slot: u64,
    ts: i64,
    n: u64,
    okn: u64,
    ro_refused: u64,
}
impl World {
    fn key(&mut self) -> K {
        self.n += 1;
        let mut b = [0u8; 32];
        b[..8].copy_from_slice(&self.n.to_le_bytes());
        b[31] = 0xaa;
        b
    }
    fn add(&mut self, owner: &Address, data: &[u8]) -> K {
        let k = self.key();
        self.accts.insert(k, Acct::new(&ad(&k), owner, data));
        k
    }
    fn signer(&mut self) -> K {
        self.add(&Address::default(), &[])
    }
    fn prog(&mut self, size: usize) -> K {
        let pid = self.pid.clone();
        self.add(&pid, &vec![0u8; size])
    }
    /// metas: (key, signer, writable). Ok => state kept; Err => rolled back.
    fn call(&mut self, metas: &[Meta], data: &[u8]) -> bool {
        *perp_program::HOST_CLOCK.lock().unwrap() = (self.slot, self.ts);
        let mut snap: Vec<(K, Vec<u8>, bool)> = vec![];
        for (k, _, wr) in metas {
            if let Some(e) = snap.iter_mut().find(|(x, _, _)| x == k) {
                e.2 |= *wr;
                continue;
            }
            if let Some(a) = self.accts.get(k) {
                snap.push((*k, a.data().to_vec(), *wr));
            } else {
                return false;
            }
        }
        let mut views: Vec<AccountView> = Vec::with_capacity(metas.len());
        for (k, s, w) in metas {
            let a = self.accts.get_mut(k).unwrap();
            let ra = a.ra();
            unsafe {
                (*ra).is_signer |= *s as u8;
                (*ra).is_writable |= *w as u8;
                views.push(AccountView::new_unchecked(ra));
            }
        }
        let r = perp_program::process_instruction(&self.pid, &mut views, data);
        if dbg() {
            eprintln!("  ix {} -> {:?}", data[0], r);
        }
        for (k, _, _) in metas {
            let ra = self.accts.get_mut(k).unwrap().ra();
            unsafe {
                (*ra).is_signer = 0;
                (*ra).is_writable = 0;
            }
        }
        let mut ok = r.is_ok();
        // the runtime refuses a transaction that changed an account it was given read-only
        if ok && snap.iter().any(|(k, d, wr)| !*wr && self.accts[k].data() != &d[..]) {
            ok = false;
            self.ro_refused += 1;
            if dbg() {
                eprintln!("  (read-only account modified: refused)");
            }
        }
        if !ok {
            for (k, d, _) in snap {
                self.accts.get_mut(&k).unwrap().data_mut().copy_from_slice(&d);
            }
        }
        if ok {
            self.okn += 1;
        }
        ok
    }
    fn rf<T>(&self, k: &K) -> &T {
        let d = self.accts[k].data();
        assert!(d.len() >= core::mem::size_of::<T>());
        unsafe { &*(d.as_ptr() as *const T) }
    }
    fn cp<T>(&self, k: &K) -> T {
        unsafe { std::ptr::read_unaligned(self.accts[k].data().as_ptr() as *const T) }
    }
    fn bytes(&self, k: &K) -> Vec<u8> {
        self.accts[k].data().to_vec()
    }
}
fn token_acct(mint: &K, owner: &K) -> Vec<u8> {
    let mut d = vec![0u8; 165];
    d[0..32].copy_from_slice(mint);
    d[32..64].copy_from_slice(owner);
    d[108] = 1;
    d
}
fn pyth(feed: &[u8; 32], price: i64, conf: u64, ts: i64) -> Vec<u8> {
    let mut d = vec![0u8; 133];
    d[0..8].copy_from_slice(&[34, 241, 35, 99, 157, 126, 244, 205]);
    d[40] = 1;
    d[41..73].copy_from_slice(feed);
    d[73..81].copy_from_slice(&price.to_le_bytes());
    d[81..89].copy_from_slice(&conf.to_le_bytes());
    d[89..93].copy_from_slice(&(-8i32).to_le_bytes());
    d[93..101].copy_from_slice(&ts.to_le_bytes());
    d[101..109].copy_from_slice(&ts.to_le_bytes());
    d
}
/// the SPL transfer is a no-op on the host: keep the vault token account's amount in step (the TVL cap reads it)
fn set_vault(w: &mut World, vault: &K, amt: i128) {
    w.accts.get_mut(vault).unwrap().data_mut()[64..72].copy_from_slice(&(amt.max(0) as u64).to_le_bytes());
}
fn le(tag: u8, v: u64) -> Vec<u8> {
    let mut d = vec![tag];
    d.extend_from_slice(&v.to_le_bytes());
    d
}
fn qitem(d: &mut Vec<u8>, idx: u16, px: u32, qty: u32) {
    d.extend_from_slice(&idx.to_le_bytes());
    d.extend_from_slice(&px.to_le_bytes());
    d.extend_from_slice(&qty.to_le_bytes());
}
