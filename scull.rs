//! A Rust port of the 'scull' (Simple Character Utility for Loading and Unloading)
//! example driver from the "Linux Device Drivers" book.
//!
//! This simplified version uses miscdevice for ease of use.

// #![no_std]


use kernel::{
    alloc::{flags::GFP_KERNEL, KBox, KVec},
    fs::file::File,
    iov::{IovIterDest, IovIterSource},
    miscdevice::{MiscDevice, MiscDeviceOptions, MiscDeviceRegistration},
    new_mutex,
    prelude::*,
    sync::{Arc, ArcBorrow, Mutex},
};

module! {
    type: ScullModule,
    name: "scull_rust",
    authors: ["Alessandro Rubini, Jonathan Corbet (Ported to Rust)"],
    description: "Rust port of the Linux Device Drivers scull example",
    license: "Dual BSD/GPL",
}
const SCULL_QUANTUM_DEFAULT: usize = 4000;
const SCULL_QSET_DEFAULT: usize = 1000;

// --- Data Structures ---

/// Represents a "quantum" - a single block of data.
type Quantum = KVec<u8>;

/// Represents a "qset" - an array of quanta.
type QSet = KVec<Option<Quantum>>;

/// Represents a node in the linked list of qsets.
struct ScullQset {
    data: Option<QSet>,
    next: Option<KBox<ScullQset>>,
}

/// Represents the data held by a single scull device.
struct ScullDevData {
    data: Option<KBox<ScullQset>>, // Head of the qset list
    quantum: usize,
    qset: usize,
    size: u64,
}


impl ScullDevData {
    fn new() -> Self {
        ScullDevData {
            data: None,
            quantum: SCULL_QUANTUM_DEFAULT,
            qset: SCULL_QSET_DEFAULT,
            size: 0,
        }
    }

    /// Empties the device.
    fn trim(&mut self) {
        let mut current = self.data.take();

        while let Some(mut qset_node) = current {
            if let Some(data_array) = qset_node.data.take() {
                for _quantum in data_array.into_iter().flatten() {
                    // Quantum is dropped here
                }
            }
            current = qset_node.next.take();
        }

        self.size = 0;
        self.quantum = SCULL_QUANTUM_DEFAULT;
        self.qset = SCULL_QSET_DEFAULT;
    }

    fn follow(&mut self, item: usize) -> Result<&mut ScullQset> {
        let current = &mut self.data;

        // Allocate first qset if needed
        if current.is_none() {
            *current = Some(KBox::new(
                ScullQset {
                    data: None,
                    next: None,
                },
                GFP_KERNEL,
            )?);
        }

        let mut current_node = current.as_mut().unwrap();

        // Follow the list `item` times
        for _ in 0..item {
            if current_node.next.is_none() {
                current_node.next = Some(KBox::new(
                    ScullQset {
                        data: None,
                        next: None,
                    },
                    GFP_KERNEL,
                )?);
            }
            current_node = current_node.next.as_mut().unwrap();
        }

        Ok(&mut **current_node)
    }
}

// --- Device Implementation ---

struct RustScull;

#[vtable]
impl MiscDevice for RustScull {
    type Ptr = Arc<Mutex<ScullDevData>>;

    fn open(_file: &File, _misc: &MiscDeviceRegistration<Self>) -> Result<Self::Ptr> {
        pr_debug!("rust_scull: open()\n");

        let data = Arc::pin_init(new_mutex!(ScullDevData::new(), "ScullDevData"), GFP_KERNEL)?;


        // Note: We can't easily check for O_WRONLY here without file flags access
        // This is a limitation of the current API

        Ok(data)
    }

    fn release(device: Self::Ptr, _file: &File) {
        pr_debug!("rust_scull: release()\n");
        // Device data is automatically dropped when Arc count reaches 0
        drop(device);
    }

    fn read_iter(
        kiocb: kernel::fs::Kiocb<'_, Self::Ptr>,
        iov: &mut IovIterDest<'_>,
    ) -> Result<usize> {
        let offset = kiocb.ki_pos() as u64;
        let device = kiocb.file();
        let inner = device.lock();

        let itemsize = inner.quantum * inner.qset;

        // Check for EOF
        if offset >= inner.size {
            return Ok(0);
        }

        if itemsize == 0 {
            return Err(EFAULT);
        }

        // Calculate how much to read
        let mut count = iov.len();
        if offset + count as u64 > inner.size {
            count = (inner.size - offset) as usize;
        }

        // Find position
        let item = (offset / itemsize as u64) as usize;
        let rest = offset % itemsize as u64;
        let s_pos = (rest / inner.quantum as u64) as usize;
        let q_pos = (rest % inner.quantum as u64) as usize;

        // Follow the list (read-only, no allocation)
        let mut dptr = inner.data.as_ref();
        for _ in 0..item {
            match dptr {
                Some(node) => dptr = node.next.as_ref(),
                None => return Ok(0),
            }
        }

        // Get the quantum
        let quantum_buf = match dptr
            .and_then(|node| node.data.as_ref())
            .and_then(|data_array| data_array.get(s_pos))
            .and_then(|quantum_opt| quantum_opt.as_ref())
        {
            Some(buf) => buf,
            None => return Ok(0),
        };

        // Read only up to the end of this quantum
        if count > inner.quantum - q_pos {
            count = inner.quantum - q_pos;
        }

        let end = (q_pos + count).min(quantum_buf.len());
        let slice_to_read = &quantum_buf[q_pos..end];

        // Copy data to user space
        iov.copy_to_iter(slice_to_read);

        Ok(slice_to_read.len())
    }

    fn write_iter(
        kiocb: kernel::fs::Kiocb<'_, Self::Ptr>,
        iov: &mut IovIterSource<'_>,
    ) -> Result<usize> {
        let offset = kiocb.ki_pos() as u64;
        let device = kiocb.file();
        let mut inner = device.lock();

        // cache fields so we don't need to borrow `inner` later
        let quantum = inner.quantum;
        let qset = inner.qset;

        let itemsize = quantum * qset;

        if itemsize == 0 {
            return Err(EFAULT);
        }

        let count = iov.len();

        // Find position using cached values
        let item = (offset / itemsize as u64) as usize;
        let rest = offset % itemsize as u64;
        let s_pos = (rest / quantum as u64) as usize;
        let q_pos = (rest % quantum as u64) as usize;

        
        let written_total: usize;
        {
            let dptr = inner.follow(item)?; 

            if dptr.data.is_none() {
                let mut qset_vec = KVec::new();
                while qset_vec.len() < qset {
                    qset_vec.push(None, GFP_KERNEL)?;
                }
                dptr.data = Some(qset_vec);
            }
            let data_array = dptr.data.as_mut().unwrap();

            if data_array[s_pos].is_none() {
                let mut quantum_vec = KVec::new();
                quantum_vec.resize(quantum, 0, GFP_KERNEL)?;
                data_array[s_pos] = Some(quantum_vec);
            }
            let quantum_buf = data_array[s_pos].as_mut().unwrap();

            let mut write_count = count;
            if write_count > quantum - q_pos {
                write_count = quantum - q_pos;
            }

            let slice_to_write = &mut quantum_buf[q_pos..q_pos + write_count];

            let copied = iov.copy_from_iter(slice_to_write);
            written_total = copied; 
        } 

        let new_offset = offset + written_total as u64;
        if inner.size < new_offset {
            inner.size = new_offset;
        }

        Ok(written_total)
    }

    // fn write_iter(
    //     kiocb: kernel::fs::Kiocb<'_, Self::Ptr>,
    //     iov: &mut IovIterSource<'_>,
    // ) -> Result<usize> {
    //     let offset = kiocb.ki_pos() as u64;
    //     let device = kiocb.file();
    //     let mut inner = device.lock();

    //     let itemsize = inner.quantum * inner.qset;

    //     if itemsize == 0 {
    //         return Err(EFAULT);
    //     }

    //     let count = iov.len();

    //     // Find position
    //     let item = (offset / itemsize as u64) as usize;
    //     let rest = offset % itemsize as u64;
    //     let s_pos = (rest / inner.quantum as u64) as usize;
    //     let q_pos = (rest % inner.quantum as u64) as usize;

    //     // Follow the list up to the right position (allocating as we go)
    //     let dptr = inner.follow(item)?;

    //     // Allocate the qset array if needed
    //     if dptr.data.is_none() {
    //         let mut qset_vec = KVec::new();
    //         while qset_vec.len() < inner.qset {
    //             qset_vec.push(None, GFP_KERNEL)?;
    //         }
    //         dptr.data = Some(qset_vec);
    //     }
    //     let data_array = dptr.data.as_mut().unwrap();

    //     // Allocate the quantum if needed
    //     if data_array[s_pos].is_none() {
    //         let mut quantum_vec = KVec::new();
    //         quantum_vec.resize(inner.quantum, 0, GFP_KERNEL)?;
    //         data_array[s_pos] = Some(quantum_vec);
    //     }
    //     let quantum_buf = data_array[s_pos].as_mut().unwrap();

    //     // Write only up to the end of this quantum
    //     let mut write_count = count;
    //     if write_count > inner.quantum - q_pos {
    //         write_count = inner.quantum - q_pos;
    //     }

    //     let slice_to_write = &mut quantum_buf[q_pos..q_pos + write_count];

    //     // Copy data from user space
    //     iov.copy_from_iter(slice_to_write);

    //     let new_offset = offset + write_count as u64;
    //     if inner.size < new_offset {
    //         inner.size = new_offset;
    //     }

    //     Ok(write_count)
    // }

    fn ioctl(
        device: ArcBorrow<'_, Mutex<ScullDevData>>,
        _file: &File,
        cmd: u32,
        arg: usize,
    ) -> Result<isize> {
        pr_debug!("rust_scull: ioctl() cmd={}, arg={}\n", cmd, arg);

        // Basic ioctl handling
        // For a full implementation, you would need to define ioctl commands
        // using kernel::ioctl macros

        match cmd {
            // Example: Reset device
            0 => {
                let mut inner = device.lock();
                inner.trim();
                Ok(0)
            }
            _ => Err(ENOTTY),
        }
    }
}

// --- Module Implementation ---

struct ScullModule {
    _dev: Pin<KBox<MiscDeviceRegistration<RustScull>>>,
}

impl kernel::Module for ScullModule {
    fn init(_module: &'static ThisModule) -> Result<Self> {
        pr_info!("rust_scull: Initializing module.\n");

        let options = MiscDeviceOptions {
            name: kernel::c_str!("scull"),
        };
        
        let dev = KBox::pin_init(MiscDeviceRegistration::register(options), GFP_KERNEL)?;

        pr_info!("rust_scull: Module initialized. Device: /dev/scull\n");

        Ok(ScullModule { _dev: dev })
    }
}

impl Drop for ScullModule {
    fn drop(&mut self) {
        pr_info!("rust_scull: Module cleanup complete.\n");
    }
}