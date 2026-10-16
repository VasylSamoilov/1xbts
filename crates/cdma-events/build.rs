fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        // A1SignalingEvent carries the SDU and the full IE list, so it dwarfs
        // the other arms. Box it to keep NetworkEvent small.
        .boxed("events.v1.NetworkEvent.body.a1")
        .compile_protos(
            &[
                "../../proto/events/v1/service.proto",
                "../../proto/events/v1/pdsn.proto",
                "../../proto/events/v1/bsc.proto",
                "../../proto/events/v1/msc.proto",
                "../../proto/events/v1/pcf.proto",
                "../../proto/events/v1/hlr.proto",
                "../../proto/events/v1/smsc.proto",
                "../../proto/events/v1/bts.proto",
                "../../proto/events/v1/an.proto",
                "../../proto/events/v1/cell.proto",
                "../../proto/events/v1/femto.proto",
                "../../proto/events/v1/a1.proto",
            ],
            &["../../proto"],
        )?;
    Ok(())
}
