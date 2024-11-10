pub mod asn1 {
    include!(concat!(env!("OUT_DIR"), "/asn1.rs"));
}

pub mod mtsid {
    include!(concat!(env!("OUT_DIR"), "/mtsid.rs"));
}

pub mod mediacontrol {
    include!(concat!(env!("OUT_DIR"), "/mediacontrol.rs"));
}

pub mod callsig {
    include!(concat!(env!("OUT_DIR"), "/callsig.rs"));
}
