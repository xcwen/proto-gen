# Protobuf compiler imports

`google/protobuf/descriptor.proto` is the unmodified Protocol Buffers v35.0 source:

https://github.com/protocolbuffers/protobuf/blob/v35.0/src/google/protobuf/descriptor.proto

It is used only to compile `common/api_options.proto` during generation. It is
not included in application PHP messages. `../download_protoc.sh` updates this
file together with the matching compiler. See `LICENSE.protobuf` for its license.
