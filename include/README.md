# Protobuf compiler imports

These files are unmodified Protocol Buffers v35.0 sources:

- https://github.com/protocolbuffers/protobuf/blob/v35.0/src/google/protobuf/descriptor.proto
- https://github.com/protocolbuffers/protobuf/blob/v35.0/src/google/protobuf/struct.proto

`descriptor.proto` compiles the common options during generation; it is not included
in application PHP messages. `struct.proto` provides `google.protobuf.Value`,
`Struct`, and `ListValue` for native JSON fields. Both schemas are embedded in the
Rust binary and written to the compilation staging directory, so a prebuilt
generator does not depend on this source directory or system protobuf includes
at runtime. See `LICENSE.protobuf` for their license.
