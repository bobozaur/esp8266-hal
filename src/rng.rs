use esp8266::RNG;
use void::Void;

pub trait RngExt {
    fn rng(self) -> Rng;
}

impl RngExt for RNG {
    fn rng(self) -> Rng {
        Rng { _rng: self }
    }
}

pub struct Rng {
    _rng: RNG,
}

impl embedded_hal::blocking::rng::Read for Rng {
    type Error = Void;

    fn read(&mut self, buffer: &mut [u8]) -> Result<(), Self::Error> {
        let rng_reg = unsafe { &*esp8266::RNG::PTR };
        for byte in buffer.iter_mut() {
            *byte = rng_reg.rng().read().bits() as u8;
        }

        Ok(())
    }
}
