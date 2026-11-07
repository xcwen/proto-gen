# Protobuf compiler imports

`google/protobuf/descriptor.proto` is the unmodified Protocol Buffers v35.0 source:

https://github.com/protocolbuffers/protobuf/blob/v35.0/src/google/protobuf/descriptor.proto

It is used only to compile `common/api_options.proto` during generation. It is
not included in application PHP messages. The schema is embedded in the Rust binary, so a prebuilt generator does not depend
on this source directory at runtime. See `LICENSE.protobuf` for its license.
