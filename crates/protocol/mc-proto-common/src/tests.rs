use crate::{MinecraftWireCodec, PacketReader, PacketWriter, WireCodec};
use bytes::BytesMut;

#[test]
fn wire_codec_round_trip_frame() {
    let codec = MinecraftWireCodec;
    let payload = vec![0x01, 0x02, 0x03];
    let frame = codec.encode_frame(&payload).expect("frame should encode");
    let mut buffer = BytesMut::from(frame.as_slice());
    let decoded = codec
        .try_decode_frame(&mut buffer)
        .expect("frame should decode")
        .expect("complete frame should be present");
    assert_eq!(decoded, payload);
    assert!(buffer.is_empty());
}

#[test]
fn packet_primitives_round_trip() {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x2a);
    writer.write_string("hello").expect("string should encode");
    writer.write_f64(12.5);
    let bytes = writer.into_inner();

    let mut reader = PacketReader::new(&bytes);
    assert_eq!(reader.read_varint().expect("varint should decode"), 0x2a);
    assert_eq!(
        reader.read_string(16).expect("string should decode"),
        "hello"
    );
    assert!((reader.read_f64().expect("double should decode") - 12.5).abs() < f64::EPSILON);
    assert!(reader.is_exhausted());
}
