//! Cadastru – bibliotecă GIS: model de date, sisteme de coordonate, formate și operații raster.

extern crate self as cadastru;

pub mod area;
pub mod convert;
pub mod crs;
pub mod expr;
pub mod formats;
pub mod geomio;
pub mod i18n;
pub mod model;
pub mod query;
pub mod rasterops;
pub mod parcel;
pub mod transdatro;

pub use crs::{Crs, Transformer};
pub use model::*;
