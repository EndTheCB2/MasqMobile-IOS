// Copyright (c) 2019, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.
use std::io;
use std::io::Read;
use std::io::Write;
use std::marker::Send;
use std::net::SocketAddr;
use std::net::TcpListener as StdTcpListener;
use tokio::io::ReadHalf;
use tokio::io::WriteHalf;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::prelude::Async;
use tokio::prelude::AsyncRead;
use tokio::prelude::AsyncWrite;
use tokio::reactor::Handle;

pub trait TokioListenerWrapper: Send {
    fn bind(&mut self, addr: SocketAddr) -> io::Result<()>;
    fn poll_accept(&mut self) -> Result<Async<(TcpStream, SocketAddr)>, io::Error>;
}

pub trait ReadHalfWrapper: Send + AsyncRead {}

pub trait WriteHalfWrapper: Send + AsyncWrite {}

pub trait TokioListenerWrapperFactory {
    fn make(&self) -> Box<dyn TokioListenerWrapper>;
}

#[derive(Default)]
pub struct TokioListenerWrapperReal {
    delegate: Option<TcpListener>,
    pending_listener: Option<StdTcpListener>,
}

pub struct ReadHalfWrapperReal {
    delegate: ReadHalf<TcpStream>,
}

pub struct WriteHalfWrapperReal {
    delegate: WriteHalf<TcpStream>,
}

pub struct TokioListenerWrapperFactoryReal {}

impl TokioListenerWrapper for TokioListenerWrapperReal {
    fn bind(&mut self, addr: SocketAddr) -> io::Result<()> {
        // Binding happens before Actix enters its Tokio runtime. Bind the OS socket now so
        // configuration errors remain synchronous, then register it with the active reactor on
        // the first poll. Tokio 0.1's direct bind otherwise fails on modern Apple platforms.
        let listener = StdTcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;
        self.pending_listener = Some(listener);
        Ok(())
    }

    fn poll_accept(&mut self) -> Result<Async<(TcpStream, SocketAddr)>, io::Error> {
        if self.delegate.is_none() {
            let listener = self
                .pending_listener
                .take()
                .expect("TcpListener not initialized - bind to a SocketAddr");
            self.delegate = Some(TcpListener::from_std(listener, &Handle::default())?);
        }
        self.delegate_mut().poll_accept()
    }
}

impl Read for ReadHalfWrapperReal {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        self.delegate.read(buf)
    }
}

impl AsyncRead for ReadHalfWrapperReal {
    fn poll_read(&mut self, buf: &mut [u8]) -> Result<Async<usize>, io::Error> {
        self.delegate.poll_read(buf)
    }
}

impl ReadHalfWrapper for ReadHalfWrapperReal {}

impl ReadHalfWrapperReal {
    pub fn new(reader: ReadHalf<TcpStream>) -> ReadHalfWrapperReal {
        ReadHalfWrapperReal { delegate: reader }
    }
}

impl Write for WriteHalfWrapperReal {
    fn write(&mut self, buf: &[u8]) -> Result<usize, io::Error> {
        self.delegate.write(buf)
    }

    fn flush(&mut self) -> Result<(), io::Error> {
        self.delegate.flush()
    }
}

impl AsyncWrite for WriteHalfWrapperReal {
    fn poll_write(&mut self, buf: &[u8]) -> Result<Async<usize>, io::Error> {
        self.delegate.poll_write(buf)
    }

    fn shutdown(&mut self) -> Result<Async<()>, io::Error> {
        self.delegate.shutdown()
    }
}

impl WriteHalfWrapper for WriteHalfWrapperReal {}

impl WriteHalfWrapperReal {
    pub fn new(writer: WriteHalf<TcpStream>) -> WriteHalfWrapperReal {
        WriteHalfWrapperReal { delegate: writer }
    }
}

impl TokioListenerWrapperFactory for TokioListenerWrapperFactoryReal {
    fn make(&self) -> Box<dyn TokioListenerWrapper> {
        Box::new(TokioListenerWrapperReal::default())
    }
}

impl TokioListenerWrapperReal {
    pub fn new() -> Self {
        Self::default()
    }

    fn delegate_mut(&mut self) -> &mut TcpListener {
        self.delegate
            .as_mut()
            .expect("TcpListener not initialized - bind to a SocketAddr")
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn nothing() {}
}
