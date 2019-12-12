use core::fmt;
use std::fmt::{Debug, Formatter};

use libc::c_void;

use spdk_sys::{spdk_bdev_free_io, spdk_bdev_io, spdk_bdev_io_complete};

use crate::bdev::{
    nexus::nexus_bdev::{Nexus, NEXUS_PRODUCT_ID},
    Bdev,
};

/// NioCtx provides context on a per IO basis
#[derive(Debug, Clone)]
pub struct NioCtx {
    /// read consistency
    pub(crate) in_flight: i8,
    /// status of the IO
    pub(crate) status: i32,
}

/// BIO is a wrapper to provides a "less unsafe" wrappers around raw
/// pointers only proper scenario testing and QA cycles can determine if this
/// code is good
///
/// We have tested this on a number of underlying devices using fio and turn on
/// verification that means that each write, is read back and checked with crc2c
///
/// other testing performed is creating a mirror of two devices and deconstruct
/// the mirror and mount the individual children without a nexus driver, and use
/// filesystem checks.
pub(crate) struct Bio {
    pub io: *mut spdk_bdev_io,
}

/// redefinition of IO types to make them (a) shorter and (b) get rid of the
/// enum conversion bloat.
///
/// The commented types are currently not used in our code base, uncomment as
/// needed.
pub mod io_type {
    pub const READ: u32 = 1;
    pub const WRITE: u32 = 2;
    pub const UNMAP: u32 = 3;
    //    pub const INVALID: u32 = 0;
    pub const FLUSH: u32 = 4;
    pub const RESET: u32 = 5;
    //    pub const NVME_ADMIN: u32 = 6;
    //    pub const NVME_IO: u32 = 7;
    //    pub const NVME_IO_MD: u32 = 8;
    //    pub const WRITE_ZEROES: u32 = 9;
    //    pub const ZCOPY: u32 = 10;
    //    pub const GET_ZONE_INFO: u32 = 11;
    //    pub const ZONE_MANAGMENT: u32 = 12;
    //    pub const ZONE_APPEND: u32 = 13;
    //    pub const IO_NUM_TYPES: u32 = 14;
}

/// the status of an IO
pub mod io_status {
    pub const NOMEM: i32 = -4;
    pub const SCSI_ERROR: i32 = -3;
    pub const NVME_ERROR: i32 = -2;
    pub const FAILED: i32 = -1;
    pub const PENDING: i32 = 0;
    pub const SUCCESS: i32 = 1;
}

impl From<*mut spdk_bdev_io> for Bio {
    fn from(io: *mut spdk_bdev_io) -> Self {
        Bio {
            io,
        }
    }
}

impl From<*mut c_void> for Bio {
    fn from(io: *mut c_void) -> Self {
        if cfg!(debug_assertions) && io.is_null() {
            panic!("bio is NULL")
        }
        Bio {
            io: io as *const _ as *mut _,
        }
    }
}

impl Bio {
    /// obtain tbe Bdev this IO is associated with
    pub(crate) fn bdev_as_ref(&self) -> Bdev {
        unsafe { Bdev::from((*self.io).bdev) }
    }

    /// complete an IO for the nexus. In the IO completion routine in
    /// `[nexus_bdev]` will set the IoStatus for each IO where success ==
    /// false.
    #[inline]
    pub(crate) fn ok(&mut self) {
        if cfg!(debug_assertions) {
            // have a child IO that has failed
            if self.io_ctx_as_mut_ref().status < 0 {
                debug!("BIO for nexus {} failed", self.nexus_as_ref().name)
            }
            // we are marking the IO done but not all child IOs have returned,
            // regardless of their state at this point
            if self.io_ctx_as_mut_ref().in_flight != 0 {
                debug!("BIO for nexus marked completed but has outstanding")
            }
        }

        unsafe { spdk_bdev_io_complete(self.io, io_status::SUCCESS) };
    }
    /// mark the IO as failed
    #[inline]
    pub(crate) fn fail(&mut self) {
        unsafe { spdk_bdev_io_complete(self.io, io_status::FAILED) };
    }

    /// asses the IO if we need to mark it failed or ok.
    #[inline]
    pub(crate) fn asses(&mut self) {
        self.io_ctx_as_mut_ref().in_flight -= 1;

        if cfg!(debug_assertions) {
            assert_ne!(self.io_ctx_as_mut_ref().in_flight, -1);
        }

        if self.io_ctx_as_mut_ref().in_flight == 0 {
            if self.io_ctx_as_mut_ref().status < io_status::PENDING {
                trace!("failing parent IO {:p} ({})", self.io, unsafe {
                    (*self.io).type_
                });
                self.fail();
            } else {
                self.ok();
            }
        }
    }

    /// obtain the Nexus struct embedded within the bdev
    pub(crate) fn nexus_as_ref(&self) -> &Nexus {
        let b = self.bdev_as_ref();
        assert_eq!(b.product_name(), NEXUS_PRODUCT_ID);
        unsafe { Nexus::from_raw((*b.inner).ctxt) }
    }

    /// get the context of the given IO, which is used to determine the overall
    /// state of the IO.
    #[inline]
    pub(crate) fn io_ctx_as_mut_ref(&mut self) -> &mut NioCtx {
        unsafe {
            &mut *((*self.io).driver_ctx.as_mut_ptr() as *const c_void
                as *mut NioCtx)
        }
    }

    /// get a raw pointer to the base of the iov
    #[inline]
    pub(crate) fn iovs(&self) -> *mut spdk_sys::iovec {
        unsafe { (*self.io).u.bdev.iovs }
    }

    /// number of iovs that are part of this IO
    #[inline]
    pub(crate) fn iov_count(&self) -> i32 {
        unsafe { (*self.io).u.bdev.iovcnt }
    }

    /// offset where we do the IO on the device
    #[inline]
    pub(crate) fn offset(&self) -> u64 {
        unsafe { (*self.io).u.bdev.offset_blocks }
    }

    /// num of blocks this IO will read/write/unmap
    #[inline]
    pub(crate) fn num_blocks(&self) -> u64 {
        unsafe { (*self.io).u.bdev.num_blocks }
    }

    /// free the io directly without completion note that the IO is not freed
    /// but rather put back into the mempool, which is allocated during startup
    #[inline]
    pub(crate) fn io_free(io: *mut spdk_bdev_io) {
        unsafe { spdk_bdev_free_io(io) }
    }

    /// determine the type of this IO
    #[inline]
    pub(crate) fn io_type(io: *mut spdk_bdev_io) -> Option<u32> {
        if io.is_null() {
            trace!("io is null!!");
            return None;
        }
        Some(unsafe { (*io).type_ } as u32)
    }

    /// get the block length of this IO
    #[inline]
    pub(crate) fn block_len(&self) -> u64 {
        unsafe { u64::from((*(*self.io).bdev).blocklen) }
    }

    /// determine if the IO needs an indirect buffer this can happen for example
    /// when we do a 512 write to a 4k device.
    #[inline]
    pub(crate) fn need_buf(&self) -> bool {
        unsafe {
            let slice = std::slice::from_raw_parts_mut(
                self.iovs(),
                self.iov_count() as usize,
            );

            slice[0].iov_base.is_null()
        }
    }
}

impl Debug for Bio {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "offset: {:?}, bytes: {:?}, type: {:?} ",
            self.offset(),
            self.num_blocks(),
            Bio::io_type(self.io).unwrap()
        )
    }
}
