use super::*;

pub(super) struct LegacyHeaderCaseIo(pub(super) tokio::net::TcpStream);

impl AsyncRead for LegacyHeaderCaseIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_read(context, buffer)
    }
}

impl AsyncWrite for LegacyHeaderCaseIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        if let Some(offset) = buffer
            .windows(SOAP_ACTION_TITLE_CASE.len())
            .position(|window| window == SOAP_ACTION_TITLE_CASE)
        {
            let mut corrected = buffer.to_vec();
            corrected[offset..offset + SOAP_ACTION_LEGACY_CASE.len()]
                .copy_from_slice(SOAP_ACTION_LEGACY_CASE);
            Pin::new(&mut self.0).poll_write(context, &corrected)
        } else {
            Pin::new(&mut self.0).poll_write(context, buffer)
        }
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.0).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.0).poll_shutdown(context)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffers: &[IoSlice<'_>],
    ) -> Poll<Result<usize, std::io::Error>> {
        let mut combined = Vec::with_capacity(buffers.iter().map(|buffer| buffer.len()).sum());
        for buffer in buffers {
            combined.extend_from_slice(buffer);
        }
        self.poll_write(context, &combined)
    }

    fn is_write_vectored(&self) -> bool {
        true
    }
}
