use ark_ec::{
    models::CurveConfig,
    short_weierstrass::{self as sw, SWCurveConfig},
};
use ark_ff::{Field, MontFp};

use crate::{fq::Fq, fr::Fr};

#[cfg(test)]
mod tests;

pub type Affine = sw::Affine<Config>;
pub type Projective = sw::Projective<Config>;

#[derive(Copy, Clone, Default, PartialEq, Eq)]
pub struct Config;

impl CurveConfig for Config {
    type BaseField = Fq;
    type ScalarField = Fr;

    /// COFACTOR = 1
    const COFACTOR: &'static [u64] = &[0x1];

    /// COFACTOR_INV = COFACTOR^{-1} mod r = 1
    const COFACTOR_INV: Fr = Fr::ONE;
}

impl SWCurveConfig for Config {
    /// COEFF_A = -3
    const COEFF_A: Fq = MontFp!("-3");

    /// COEFF_B = 27580193559959705877849011840389048093056905856361568521428707301988689241309860865136260764883745107765439761230575
    const COEFF_B: Fq =
        MontFp!("27580193559959705877849011840389048093056905856361568521428707301988689241309860865136260764883745107765439761230575");

    /// Correctness:
    /// The curve equation is y^2 = x^3 + ax + b
    /// Substituting (0, 0) gives 0^2 = 0^3 + a*0 + b which simplifies to 0 = b.
    /// Since b is not zero, the point (0, 0) is not on the curve.
    /// Therefore, we can safely use (0, 0) as a flag for the zero point.
    type ZeroFlag = ();

    /// GENERATOR = (G_GENERATOR_X, G_GENERATOR_Y)
    const GENERATOR: Affine = Affine::new_unchecked(G_GENERATOR_X, G_GENERATOR_Y);
}

/// G_GENERATOR_X =
/// 26247035095799689268623156744566981891852923491109213387815615900925518854738050089022388053975719786650872476732087
pub const G_GENERATOR_X: Fq =
    MontFp!("26247035095799689268623156744566981891852923491109213387815615900925518854738050089022388053975719786650872476732087");

/// G_GENERATOR_Y =
/// 8325710961489029985546751289520108179287853048861315594709205902480503199884419224438643760392947333078086511627871
pub const G_GENERATOR_Y: Fq =
    MontFp!("8325710961489029985546751289520108179287853048861315594709205902480503199884419224438643760392947333078086511627871");
