use flex_error::{define_error, DisplayOnly}; //, TraceError};

define_error! {
    #[derive(Debug, PartialEq, Eq)]
    Error {
        InvalidHeader
            { reason: String }
            [ DisplayOnly<Box<dyn std::error::Error + Send + Sync>> ]
            | _ | { "invalid header, failed basic validation" },
        InvalidRawClientState
            { reason: String }
            [ DisplayOnly<Box<dyn std::error::Error + Send + Sync>> ]
            | _ | { "invalid raw client state" },
        MissingRawClientState
            { reason: String }
            | _ | { "missing raw client state" },
        MissingRawConsensusState
            { reason: String }
            | _ | { "missing raw consensus state" },
    }
}
