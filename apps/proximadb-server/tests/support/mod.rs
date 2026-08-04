use std::io;
use std::net::TcpListener;

/// Holds simultaneously allocated loopback ports until a subprocess is ready
/// to bind them.
///
/// Binding every listener before collecting the port numbers prevents the OS
/// from immediately recycling one ephemeral port into another slot in the
/// same server configuration. The caller releases the reservation immediately
/// before `Command::spawn`; retaining it through the spawn would make the
/// server's binds fail with `Address already in use`.
pub(crate) struct LoopbackPortReservation<const N: usize> {
    ports: [u16; N],
    _listeners: Vec<TcpListener>,
}

impl<const N: usize> LoopbackPortReservation<N> {
    pub(crate) fn ports(&self) -> [u16; N] {
        self.ports
    }
}

pub(crate) fn reserve_loopback_ports<const N: usize>() -> io::Result<LoopbackPortReservation<N>> {
    let mut listeners = Vec::with_capacity(N);
    let mut ports = Vec::with_capacity(N);
    for _ in 0..N {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        ports.push(listener.local_addr()?.port());
        listeners.push(listener);
    }
    let ports = ports.try_into().map_err(|ports: Vec<u16>| {
        io::Error::other(format!(
            "allocated {} loopback ports, expected {N}",
            ports.len()
        ))
    })?;
    Ok(LoopbackPortReservation {
        ports,
        _listeners: listeners,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    #[test]
    fn one_reservation_cannot_reuse_a_port() -> std::io::Result<()> {
        for _ in 0..128 {
            let reservation = super::reserve_loopback_ports::<4>()?;
            let ports = reservation.ports();
            let unique = ports.into_iter().collect::<BTreeSet<_>>();
            assert_eq!(unique.len(), ports.len(), "reserved ports: {ports:?}");
        }
        Ok(())
    }
}
