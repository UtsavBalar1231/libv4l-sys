use std::ffi::{CStr, CString};
use std::fmt;
use std::fs;
use std::io::Write;
use std::mem;
use std::ptr;
use std::slice;

use log::*;

use libv4l_sys as v4l;

macro_rules! errno {
    () => {
        unsafe { *libc::__errno_location() }
    };
}

#[derive(Debug, Clone)]
struct Framesize {
    pub width: u32,
    pub height: u32,
}

impl fmt::Display for Framesize {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

#[derive(Debug)]
struct Buffer {
    start: *mut libc::c_void,
    length: libc::size_t,
}

fn strerror() -> String {
    let errno = errno!();
    unsafe { CStr::from_ptr(libc::strerror(errno)) }
        .to_string_lossy()
        .into()
}

fn rioctl(fd: libc::c_int, request: libc::c_ulong, arg: *mut libc::c_void) -> libc::c_int {
    let mut r;

    loop {
        r = unsafe { v4l::v4l2_ioctl(fd, request, arg) };
        if r == -1 && ((errno!() == libc::EINTR) || (errno!() == libc::EAGAIN)) {
            continue;
        } else {
            break;
        }
    }
    r
}

fn xioctl(fd: libc::c_int, request: libc::c_ulong, arg: *mut libc::c_void) {
    if rioctl(fd, request, arg) == -1 {
        error!("error {}, {}", errno!(), strerror());
        panic!()
    }
}

fn main() {
    println!("v4l2grab");
    env_logger::init();

    let dev_name = CString::new("/dev/video0").unwrap();

    let fd = unsafe {
        let fd = v4l::v4l2_open(dev_name.as_ptr(), libc::O_RDWR | libc::O_NONBLOCK, 0);
        if fd == -1 {
            error!("open device error: {}: {}", fd, strerror());
            panic!()
        }
        fd
    };

    let fmt = unsafe {
        let mut fmt: v4l::v4l2_format = mem::zeroed();
        fmt.type_ = v4l::v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE;
        fmt.fmt.pix.width = 640;
        fmt.fmt.pix.height = 480;
        fmt.fmt.pix.pixelformat = v4l::pixel_format::V4L2_PIX_FMT_RGB24;
        fmt.fmt.pix.field = v4l::v4l2_field_V4L2_FIELD_INTERLACED;

        xioctl(
            fd,
            v4l::codes::VIDIOC_S_FMT,
            &mut fmt as *mut _ as *mut libc::c_void,
        );
        if fmt.fmt.pix.pixelformat != v4l::pixel_format::V4L2_PIX_FMT_RGB24 {
            println!("Libv4l didn't accept RGB24 format. Can't proceed.");
            panic!()
        }
        if (fmt.fmt.pix.width != 640) || (fmt.fmt.pix.height != 480) {
            println!(
                "Warning: driver is sending image at {}x{}",
                fmt.fmt.pix.width, fmt.fmt.pix.height
            );
        }
        fmt
    };

    let mut req = unsafe {
        let mut req: v4l::v4l2_requestbuffers = mem::zeroed();
        req.count = 2;
        req.type_ = v4l::v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE;
        req.memory = v4l::v4l2_memory_V4L2_MEMORY_MMAP;
        req
    };

    xioctl(
        fd,
        v4l::codes::VIDIOC_REQBUFS,
        &mut req as *mut _ as *mut libc::c_void,
    );

    let mut buffers = Vec::new();
    for n_buffers in 0..req.count {
        let mut buf = unsafe {
            let mut buf: v4l::v4l2_buffer = mem::zeroed();
            buf.type_ = v4l::v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE;
            buf.memory = v4l::v4l2_memory_V4L2_MEMORY_MMAP;
            buf.index = n_buffers;
            buf
        };

        xioctl(
            fd,
            v4l::codes::VIDIOC_QUERYBUF,
            &mut buf as *mut _ as *mut libc::c_void,
        );
        let buffer = unsafe {
            Buffer {
                start: v4l::v4l2_mmap(
                    ptr::null_mut(),
                    buf.length.try_into().unwrap(),
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd,
                    buf.m.offset as i64,
                ),
                length: buf.length as libc::size_t,
            }
        };

        if libc::MAP_FAILED == buffer.start {
            error!("mmap");
            panic!()
        }
        buffers.push(buffer);
    }

    for i in 0..buffers.len() {
        let mut buf = unsafe {
            let mut buf: v4l::v4l2_buffer = mem::zeroed();
            buf.type_ = v4l::v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE;
            buf.memory = v4l::v4l2_memory_V4L2_MEMORY_MMAP;
            buf.index = i as u32;
            buf
        };

        xioctl(
            fd,
            v4l::codes::VIDIOC_QBUF,
            &mut buf as *mut _ as *mut libc::c_void,
        );
    }

    let mut type_ = v4l::v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE;
    xioctl(
        fd,
        v4l::codes::VIDIOC_STREAMON,
        &mut type_ as *mut _ as *mut libc::c_void,
    );

    unsafe {
        let mut fds: libc::fd_set = mem::zeroed();
        loop {
            libc::FD_ZERO(&mut fds);
            libc::FD_SET(fd, &mut fds);
            let mut tv = libc::timeval {
                tv_sec: 2,
                tv_usec: 0,
            };
            let r = libc::select(fd + 1, &mut fds, ptr::null_mut(), ptr::null_mut(), &mut tv);
            if !(r == -1 && (errno!() == libc::EINTR)) {
                if cfg!(target_os = "linux") {
                    // debug!("time left: {}.{:06}", tv.tv_sec, tv.tv_usec);
                }
                break;
            }
            if r == -1 {
                error!("select error {}, {}", errno!(), strerror());
                panic!()
            }
        }
    }

    let mut buf = unsafe {
        let mut buf: v4l::v4l2_buffer = mem::zeroed();
        buf.type_ = v4l::v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE;
        buf.memory = v4l::v4l2_memory_V4L2_MEMORY_MMAP;
        buf
    };

    // debug!("VIDIOC_DQBUF");
    xioctl(
        fd,
        v4l::codes::VIDIOC_DQBUF,
        &mut buf as *mut _ as *mut libc::c_void,
    );

    {
        let mut fout = fs::File::create(&format!("out.ppm")).unwrap();
        unsafe {
            let data = slice::from_raw_parts(
                buffers[buf.index as usize].start as *const u8,
                buf.bytesused as usize,
            );

            let vdata = data.to_vec();

            //#[derive(Debug)]
            struct FpgaFrame {
                sof: [u8; 2],
                pkt_id: [u8; 4],
                dtype: u8,
                dlen: [u8; 2],
                phl_id: [u8; 2],
                reserved: u8,
                data: Vec<u8>,
                eof: [u8; 2],
            }

            impl fmt::Display for FpgaFrame {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    writeln!(f, "FpgaFrame {{")?;
                    writeln!(f, "\tsof: {:X?}", self.sof)?;
                    writeln!(f, "\tpkt_id: {:X?}", self.pkt_id)?;
                    writeln!(f, "\tdtype: {:X?}", self.dtype)?;
                    writeln!(f, "\tdlen: {:X?}", self.dlen)?;
                    writeln!(f, "\tphl_id: {:X?}", self.phl_id)?;
                    writeln!(f, "\treserved: {:X?}", self.reserved)?;
                    writeln!(f, "\tdata: {:?}", std::str::from_utf8(&self.data))?;
                    writeln!(f, "\teof: {:X?}", self.eof)?;
                    writeln!(f, "}}")
                }
            }

            let dlen = [vdata[7], vdata[8]];
            #[allow(arithmetic_overflow)]
            let dlen_size: usize = ((dlen[0] as usize) << 8) | dlen[1] as usize;
            println!("dlen: {:#04X?}, dlen_size: {:#04X?}", dlen, dlen_size);
            let mut temp = Vec::new();

            temp.extend(&vdata[12..dlen_size]);

            let frame = FpgaFrame {
                sof: [vdata[0], vdata[1]],
                pkt_id: [vdata[2], vdata[3], vdata[4], vdata[5]],
                dtype: vdata[6],
                dlen,
                phl_id: [vdata[7], vdata[8]],
                reserved: vdata[11],
                data: temp,
                eof: [
                    vdata[dlen_size + 12 as usize],
                    vdata[dlen_size + 13 as usize],
                ],
                /*
                    sof: (vdata[0] << 8) + vdata[1],
                    pkt_id: ((vdata[2] << 24) + (vdata[3] << 16) + (vdata[4] << 8) + vdata[5]),
                    dtype: vdata[6],
                    dlen: dlen,
                    phl_id: (vdata[9] << 8) + vdata[10],
                    reserved: vdata[11],
                    data: &temp,
                    eof: (vdata[i+1+12] << 8) + vdata[i+2+12],
                */
            };

            println!("{}", frame);

            //		println!("data: {:#?}", core::str::from_utf8_unchecked(data));
            println!("data len: {:#?}", data.len());
            fout.write_all(data).unwrap();
        }
    }

    // debug!("VIDIOC_QBUF");
    xioctl(
        fd,
        v4l::codes::VIDIOC_QBUF,
        &mut buf as *mut _ as *mut libc::c_void,
    );

    let mut type_ = v4l::v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE;
    // debug!("VIDIOC_STREAMOFF");
    xioctl(
        fd,
        v4l::codes::VIDIOC_STREAMOFF,
        &mut type_ as *mut _ as *mut libc::c_void,
    );

    unsafe {
        for buf in buffers {
            v4l::v4l2_munmap(buf.start, buf.length);
        }
        v4l::v4l2_close(fd);
    }
}
