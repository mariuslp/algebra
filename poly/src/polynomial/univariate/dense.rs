//! A dense univariate polynomial represented in coefficient form.
use crate::{
    univariate::{DenseOrSparsePolynomial, SparsePolynomial},
    DenseUVPolynomial, EvaluationDomain, Evaluations, GeneralEvaluationDomain, Polynomial,
};
use ark_ff::{FftField, Field, Zero};
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use ark_std::{
    cfg_iter_mut, fmt,
    ops::{Add, AddAssign, Deref, DerefMut, Div, Mul, Neg, Sub, SubAssign},
    rand::Rng,
    vec,
    vec::*,
};

#[cfg(feature = "parallel")]
use ark_std::cmp::max;
#[cfg(feature = "parallel")]
use rayon::prelude::*;

/// Stores a polynomial in coefficient form.
#[derive(Clone, PartialEq, Eq, Hash, Default, CanonicalSerialize, CanonicalDeserialize)]
pub struct DensePolynomial<F: Field> {
    /// The coefficient of `x^i` is stored at location `i` in `self.coeffs`.
    pub coeffs: Vec<F>,
}

impl<F: Field> Polynomial<F> for DensePolynomial<F> {
    type Point = F;

    /// Returns the total degree of the polynomial
    fn degree(&self) -> usize {
        if self.is_zero() {
            0
        } else {
            assert!(self.coeffs.last().is_some_and(|coeff| !coeff.is_zero()));
            self.coeffs.len() - 1
        }
    }

    /// Evaluates `self` at the given `point` in `Self::Point`.
    fn evaluate(&self, point: &F) -> F {
        if self.is_zero() {
            F::zero()
        } else if point.is_zero() {
            self.coeffs[0]
        } else {
            self.internal_evaluate(point)
        }
    }
}

#[cfg(feature = "parallel")]
// Set some minimum number of field elements to be worked on per thread
// to avoid per-thread costs dominating parallel execution time.
const MIN_ELEMENTS_PER_THREAD: usize = 16;

impl<F: Field> DensePolynomial<F> {
    #[inline]
    // Horner's method for polynomial evaluation
    fn horner_evaluate(poly_coeffs: &[F], point: &F) -> F {
        poly_coeffs
            .iter()
            .rfold(F::zero(), move |result, coeff| result * point + coeff)
    }

    #[cfg(not(feature = "parallel"))]
    fn internal_evaluate(&self, point: &F) -> F {
        Self::horner_evaluate(&self.coeffs, point)
    }

    #[cfg(feature = "parallel")]
    fn internal_evaluate(&self, point: &F) -> F {
        // Horners method - parallel method
        // compute the number of threads we will be using.
        let num_cpus_available = rayon::current_num_threads();
        let num_coeffs = self.coeffs.len();
        let num_elem_per_thread = max(num_coeffs / num_cpus_available, MIN_ELEMENTS_PER_THREAD);

        // run Horners method on each thread as follows:
        // 1) Split up the coefficients across each thread evenly.
        // 2) Do polynomial evaluation via horner's method for the thread's coefficients
        // 3) Scale the result point^{thread coefficient start index}
        // Then obtain the final polynomial evaluation by summing each threads result.
        self.coeffs
            .par_chunks(num_elem_per_thread)
            .enumerate()
            .map(|(i, chunk)| {
                Self::horner_evaluate(chunk, point) * point.pow([(i * num_elem_per_thread) as u64])
            })
            .sum()
    }
}

impl<F: Field> DenseUVPolynomial<F> for DensePolynomial<F> {
    /// Constructs a new polynomial from a list of coefficients.
    fn from_coefficients_slice(coeffs: &[F]) -> Self {
        Self::from_coefficients_vec(coeffs.to_vec())
    }

    /// Constructs a new polynomial from a list of coefficients.
    fn from_coefficients_vec(coeffs: Vec<F>) -> Self {
        let mut result = Self { coeffs };
        // While there are zeros at the end of the coefficient vector, pop them off.
        result.truncate_leading_zeros();
        // Check that either the coefficients vec is empty or that the last coeff is
        // non-zero.
        assert!(result.coeffs.last().map_or(true, |coeff| !coeff.is_zero()));
        result
    }

    /// Returns the coefficients of `self`
    fn coeffs(&self) -> &[F] {
        &self.coeffs
    }

    /// Outputs a univariate polynomial of degree `d` where each non-leading
    /// coefficient is sampled uniformly at random from `F` and the leading
    /// coefficient is sampled uniformly at random from among the non-zero
    /// elements of `F`.
    ///
    /// # Example
    /// ```
    /// use ark_std::test_rng;
    /// use ark_test_curves::bls12_381::Fr;
    /// use ark_poly::{univariate::DensePolynomial, Polynomial, DenseUVPolynomial};
    ///
    /// let rng = &mut test_rng();
    /// let poly = DensePolynomial::<Fr>::rand(8, rng);
    /// assert_eq!(poly.degree(), 8);
    /// ```
    fn rand<R: Rng>(d: usize, rng: &mut R) -> Self {
        let mut random_coeffs = Vec::new();

        if d > 0 {
            // d - 1 overflows when d = 0
            for _ in 0..=(d - 1) {
                random_coeffs.push(F::rand(rng));
            }
        }

        let mut leading_coefficient = F::rand(rng);

        while leading_coefficient.is_zero() {
            leading_coefficient = F::rand(rng);
        }

        random_coeffs.push(leading_coefficient);

        Self::from_coefficients_vec(random_coeffs)
    }
}

impl<F: FftField> DensePolynomial<F> {
    /// Multiply `self` by the vanishing polynomial for the domain `domain`.
    /// Returns the result of the multiplication.
    pub fn mul_by_vanishing_poly<D: EvaluationDomain<F>>(&self, domain: D) -> Self {
        let mut shifted = vec![F::zero(); domain.size()];
        shifted.extend_from_slice(&self.coeffs);
        cfg_iter_mut!(shifted)
            .zip(&self.coeffs)
            .for_each(|(s, c)| *s -= c);
        Self::from_coefficients_vec(shifted)
    }

    /// Divide `self` by the vanishing polynomial for the domain `domain`.
    /// Returns the quotient and remainder of the division.
    pub fn divide_by_vanishing_poly<D: EvaluationDomain<F>>(&self, domain: D) -> (Self, Self) {
        let domain_size = domain.size();

        if self.coeffs.len() < domain_size {
            // If degree(self) < len(Domain), then the quotient is zero, and the entire polynomial is the remainder
            (Self::zero(), self.clone())
        } else {
            // Compute the quotient
            //
            // If `self.len() <= 2 * domain_size`
            //    then quotient is simply `self.coeffs[domain_size..]`
            // Otherwise
            //    during the division by `x^domain_size - 1`, some of `self.coeffs[domain_size..]` will be updated as well
            //    which can be computed using the following algorithm.
            //
            let mut quotient_vec = self.coeffs[domain_size..].to_vec();
            for i in 1..(self.len() / domain_size) {
                cfg_iter_mut!(quotient_vec)
                    .zip(&self.coeffs[domain_size * (i + 1)..])
                    .for_each(|(s, c)| *s += c);
            }

            // Compute the remainder
            //
            // `remainder = self - quotient_vec * (x^domain_size - 1)`
            //
            // Note that remainder must be smaller than `domain_size`.
            // So we can look at only the first `domain_size` terms.
            //
            // Therefore,
            // `remainder = self.coeffs[0..domain_size] - quotient_vec * (-1)`
            // i.e.,
            // `remainder = self.coeffs[0..domain_size] + quotient_vec`
            //
            let mut remainder_vec = self.coeffs[0..domain_size].to_vec();
            cfg_iter_mut!(remainder_vec)
                .zip(&quotient_vec)
                .for_each(|(s, c)| *s += c);

            let quotient = Self::from_coefficients_vec(quotient_vec);
            let remainder = Self::from_coefficients_vec(remainder_vec);
            (quotient, remainder)
        }
    }
}

impl<F: Field> DensePolynomial<F> {
    fn truncate_leading_zeros(&mut self) {
        while self.coeffs.last().is_some_and(|c| c.is_zero()) {
            self.coeffs.pop();
        }
    }

    /// Perform a naive n^2 multiplication of `self` by `other`.
    pub fn naive_mul(&self, other: &Self) -> Self {
        if self.is_zero() || other.is_zero() {
            Self::zero()
        } else {
            let mut result = vec![F::zero(); self.degree() + other.degree() + 1];
            for (i, self_coeff) in self.coeffs.iter().enumerate() {
                for (j, other_coeff) in other.coeffs.iter().enumerate() {
                    result[i + j] += &(*self_coeff * other_coeff);
                }
            }
            Self::from_coefficients_vec(result)
        }
    }

    /// Returns the quotient of the division of `self` by `other`
    /// using a naive O(nk) algorithm, with n, k the respective degrees of
    /// the dividend and divisor
    pub fn naive_div(&self, other: &Self) -> Self {
        let dividend: DenseOrSparsePolynomial<'_, F> = self.into();
        let divisor: DenseOrSparsePolynomial<'_, F> = other.into();

        dividend.naive_div(&divisor).expect("division failed").0
    }
}

impl<F: FftField> DensePolynomial<F> {
    /// Evaluate `self` over `domain`.
    pub fn evaluate_over_domain_by_ref<D: EvaluationDomain<F>>(
        &self,
        domain: D,
    ) -> Evaluations<F, D> {
        let poly: DenseOrSparsePolynomial<'_, F> = self.into();
        DenseOrSparsePolynomial::<F>::evaluate_over_domain(poly, domain)
    }

    /// Evaluate `self` over `domain`.
    pub fn evaluate_over_domain<D: EvaluationDomain<F>>(self, domain: D) -> Evaluations<F, D> {
        let poly: DenseOrSparsePolynomial<'_, F> = self.into();
        DenseOrSparsePolynomial::<F>::evaluate_over_domain(poly, domain)
    }
}

impl<F: Field> fmt::Debug for DensePolynomial<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        for (i, coeff) in self.coeffs.iter().enumerate().filter(|(_, c)| !c.is_zero()) {
            if i == 0 {
                write!(f, "\n{:?}", coeff)?;
            } else if i == 1 {
                write!(f, " + \n{:?} * x", coeff)?;
            } else {
                write!(f, " + \n{:?} * x^{}", coeff, i)?;
            }
        }
        Ok(())
    }
}

impl<F: Field> Deref for DensePolynomial<F> {
    type Target = [F];

    fn deref(&self) -> &[F] {
        &self.coeffs
    }
}

impl<F: Field> DerefMut for DensePolynomial<F> {
    fn deref_mut(&mut self) -> &mut [F] {
        &mut self.coeffs
    }
}

impl<'a, F: Field> Add<&'a DensePolynomial<F>> for &DensePolynomial<F> {
    type Output = DensePolynomial<F>;

    fn add(self, other: &'a DensePolynomial<F>) -> DensePolynomial<F> {
        // If the first polynomial is zero, the result is simply the second polynomial.
        if self.is_zero() {
            return other.clone();
        }

        // If the second polynomial is zero, the result is simply the first polynomial.
        if other.is_zero() {
            return self.clone();
        }

        // Determine which polynomial has the higher degree.
        let (longer, shorter) = if self.degree() >= other.degree() {
            (self, other)
        } else {
            (other, self)
        };

        // Start with a copy of the longer polynomial as the base for the result.
        let mut result = longer.clone();

        // Iterate through the coefficients of the `shorter` polynomial.
        // Add them to the corresponding coefficients in the `longer` polynomial.
        cfg_iter_mut!(result)
            .zip(&shorter.coeffs)
            .for_each(|(a, b)| *a += b);

        // Remove any trailing zeros from the resulting polynomial.
        result.truncate_leading_zeros();

        result
    }
}

impl<'a, F: Field> Add<&'a SparsePolynomial<F>> for &DensePolynomial<F> {
    type Output = DensePolynomial<F>;

    #[inline]
    fn add(self, other: &'a SparsePolynomial<F>) -> DensePolynomial<F> {
        if self.is_zero() {
            return other.clone().into();
        }

        if other.is_zero() {
            return self.clone();
        }

        let mut result = self.clone();

        // Reserve space for additional coefficients if `other` has a higher degree.
        let additional_len = other.degree().saturating_sub(result.degree());
        result.coeffs.reserve(additional_len);

        // Process each term in `other`.
        for (pow, coeff) in other.iter() {
            if let Some(target) = result.coeffs.get_mut(*pow) {
                *target += coeff;
            } else {
                // Extend with zeros if the power exceeds the current length.
                result
                    .coeffs
                    .extend(ark_std::iter::repeat(F::zero()).take(pow - result.coeffs.len()));
                result.coeffs.push(*coeff);
            }
        }

        // Remove any leading zeros.
        // For example: `0 * x^2 + 0 * x + 1` should be represented as `1`.
        result.truncate_leading_zeros();

        result
    }
}

impl<'a, F: Field> AddAssign<&'a Self> for DensePolynomial<F> {
    fn add_assign(&mut self, other: &'a Self) {
        if other.is_zero() {
            self.truncate_leading_zeros();
            return;
        }

        if self.is_zero() {
            self.coeffs.clear();
            self.coeffs.extend_from_slice(&other.coeffs);
        } else {
            let other_coeffs_len = other.coeffs.len();
            if other_coeffs_len > self.coeffs.len() {
                // Add the necessary number of zero coefficients.
                self.coeffs.resize(other_coeffs_len, F::zero());
            }

            self.coeffs
                .iter_mut()
                .zip(&other.coeffs)
                .for_each(|(a, b)| *a += b);
        }
        self.truncate_leading_zeros();
    }
}

impl<'a, F: Field> AddAssign<(F, &'a Self)> for DensePolynomial<F> {
    fn add_assign(&mut self, (f, other): (F, &'a Self)) {
        // No need to modify self if other is zero
        if other.is_zero() {
            return;
        }

        // If the first polynomial is zero, just copy the second one and scale by f.
        if self.is_zero() {
            self.coeffs.clear();
            self.coeffs.extend_from_slice(&other.coeffs);
            self.coeffs.iter_mut().for_each(|c| *c *= &f);
            return;
        }

        // If the degree of the first polynomial is smaller, resize it.
        if self.degree() < other.degree() {
            self.coeffs.resize(other.coeffs.len(), F::zero());
        }

        // Add corresponding coefficients from the second polynomial, scaled by f.
        self.coeffs
            .iter_mut()
            .zip(&other.coeffs)
            .for_each(|(a, b)| *a += f * b);

        // If the leading coefficient ends up being zero, pop it off.
        // This can happen:
        // - if they were the same degree,
        // - if a polynomial's coefficients were constructed with leading zeros.
        self.truncate_leading_zeros();
    }
}

impl<'a, F: Field> AddAssign<&'a SparsePolynomial<F>> for DensePolynomial<F> {
    #[inline]
    fn add_assign(&mut self, other: &'a SparsePolynomial<F>) {
        // No need to modify self if other is zero
        if other.is_zero() {
            return;
        }

        // If the first polynomial is zero, just copy the second one.
        if self.is_zero() {
            self.coeffs.clear();
            self.coeffs.resize(other.degree() + 1, F::zero());
            for (i, coeff) in other.iter() {
                self.coeffs[*i] = *coeff;
            }
        } else {
            // If neither polynomial is zero, we proceed to add the terms.
            let lhs_degree = self.degree();

            // Resize the coefficients of the left-hand side if necessary.
            // This is done to ensure that the left-hand side has enough coefficients.
            let max_degree = lhs_degree.max(other.degree());
            self.coeffs.resize(max_degree + 1, F::zero());

            // Add the coefficients of the right-hand side to the left-hand side.
            // - For pow <= lhs_degree, add the coefficients.
            // - For pow > lhs_degree, set the coefficients (no addition is needed).
            for (pow, coeff) in other.iter() {
                if *pow <= lhs_degree {
                    self.coeffs[*pow] += coeff;
                } else {
                    self.coeffs[*pow] = *coeff;
                }
            }
        }

        // Truncate leading zeros after addition
        self.truncate_leading_zeros();
    }
}

impl<F: Field> Neg for DensePolynomial<F> {
    type Output = Self;

    #[inline]
    fn neg(mut self) -> Self {
        self.coeffs.iter_mut().for_each(|coeff| {
            *coeff = -*coeff;
        });
        self
    }
}

impl<'a, F: Field> Sub<&'a DensePolynomial<F>> for &DensePolynomial<F> {
    type Output = DensePolynomial<F>;

    #[inline]
    fn sub(self, other: &'a DensePolynomial<F>) -> DensePolynomial<F> {
        let mut result = if self.is_zero() {
            let mut result = other.clone();
            result.coeffs.iter_mut().for_each(|c| *c = -(*c));
            result
        } else if other.is_zero() {
            self.clone()
        } else if self.degree() >= other.degree() {
            let mut result = self.clone();
            result
                .coeffs
                .iter_mut()
                .zip(&other.coeffs)
                .for_each(|(a, b)| *a -= b);
            result
        } else {
            let mut result = self.clone();
            result.coeffs.resize(other.coeffs.len(), F::zero());
            result
                .coeffs
                .iter_mut()
                .zip(&other.coeffs)
                .for_each(|(a, b)| *a -= b);
            result
        };
        result.truncate_leading_zeros();
        result
    }
}

impl<'a, F: Field> Sub<&'a SparsePolynomial<F>> for &DensePolynomial<F> {
    type Output = DensePolynomial<F>;

    #[inline]
    fn sub(self, other: &'a SparsePolynomial<F>) -> DensePolynomial<F> {
        if self.is_zero() {
            let result = other.clone();
            (-result).into()
        } else if other.is_zero() {
            self.clone()
        } else {
            let mut result = self.clone();
            // If `other` has higher degree than `self`, create a dense vector
            // storing the upper coefficients of the subtraction
            let mut upper_coeffs = match other.degree() > result.degree() {
                true => vec![F::zero(); other.degree() - result.degree()],
                false => Vec::new(),
            };
            for (pow, coeff) in other.iter() {
                if *pow <= result.degree() {
                    result.coeffs[*pow] -= coeff;
                } else {
                    upper_coeffs[*pow - result.degree() - 1] = -*coeff;
                }
            }
            result.coeffs.extend(upper_coeffs);
            result
        }
    }
}

impl<'a, F: Field> SubAssign<&'a Self> for DensePolynomial<F> {
    #[inline]
    fn sub_assign(&mut self, other: &'a Self) {
        if self.is_zero() {
            self.coeffs.resize(other.coeffs.len(), F::zero());
        } else if other.is_zero() {
            return;
        } else if self.degree() >= other.degree() {
        } else {
            // Add the necessary number of zero coefficients.
            self.coeffs.resize(other.coeffs.len(), F::zero());
        }
        self.coeffs
            .iter_mut()
            .zip(&other.coeffs)
            .for_each(|(a, b)| {
                *a -= b;
            });
        // If the leading coefficient ends up being zero, pop it off.
        // This can happen if they were the same degree, or if other's
        // coefficients were constructed with leading zeros.
        self.truncate_leading_zeros();
    }
}

impl<'a, F: Field> SubAssign<&'a SparsePolynomial<F>> for DensePolynomial<F> {
    #[inline]
    fn sub_assign(&mut self, other: &'a SparsePolynomial<F>) {
        if self.is_zero() {
            self.coeffs.truncate(0);
            self.coeffs.resize(other.degree() + 1, F::zero());

            for (i, coeff) in other.iter() {
                self.coeffs[*i] = (*coeff).neg();
            }
        } else if other.is_zero() {
        } else {
            // If `other` has higher degree than `self`, create a dense vector
            // storing the upper coefficients of the subtraction
            let mut upper_coeffs = match other.degree() > self.degree() {
                true => vec![F::zero(); other.degree() - self.degree()],
                false => Vec::new(),
            };
            for (pow, coeff) in other.iter() {
                if *pow <= self.degree() {
                    self.coeffs[*pow] -= coeff;
                } else {
                    upper_coeffs[*pow - self.degree() - 1] = -*coeff;
                }
            }
            self.coeffs.extend(upper_coeffs);
        }
    }
}

impl<'a, F: FftField> Div<&'a DensePolynomial<F>> for &DensePolynomial<F> {
    type Output = DensePolynomial<F>;

    #[inline]
    fn div(self, divisor: &'a DensePolynomial<F>) -> DensePolynomial<F> {
        let a = DenseOrSparsePolynomial::from(self);
        let b = DenseOrSparsePolynomial::from(divisor);
        a.divide(&b).expect("division failed")
    }
}

impl<F: Field> Mul<F> for &DensePolynomial<F> {
    type Output = DensePolynomial<F>;

    #[inline]
    fn mul(self, elem: F) -> DensePolynomial<F> {
        if self.is_zero() || elem.is_zero() {
            DensePolynomial::zero()
        } else {
            let mut result = self.clone();
            cfg_iter_mut!(result).for_each(|e| {
                *e *= elem;
            });
            result
        }
    }
}

impl<F: Field> Mul<F> for DensePolynomial<F> {
    type Output = Self;

    #[inline]
    fn mul(self, elem: F) -> Self {
        &self * elem
    }
}

/// Performs O(nlogn) multiplication of polynomials if F is smooth.
impl<'a, F: FftField> Mul<&'a DensePolynomial<F>> for &DensePolynomial<F> {
    type Output = DensePolynomial<F>;

    #[inline]
    fn mul(self, other: &'a DensePolynomial<F>) -> DensePolynomial<F> {
        if self.is_zero() || other.is_zero() {
            DensePolynomial::zero()
        } else {
            let domain = GeneralEvaluationDomain::new(self.coeffs.len() + other.coeffs.len() - 1)
                .expect("field is not smooth enough to construct domain");
            let mut self_evals = self.evaluate_over_domain_by_ref(domain);
            let other_evals = other.evaluate_over_domain_by_ref(domain);
            self_evals *= &other_evals;
            self_evals.interpolate()
        }
    }
}

macro_rules! impl_op {
    ($trait:ident, $method:ident, $field_bound:ident) => {
        impl<F: $field_bound> $trait<DensePolynomial<F>> for DensePolynomial<F> {
            type Output = DensePolynomial<F>;

            #[inline]
            fn $method(self, other: DensePolynomial<F>) -> DensePolynomial<F> {
                (&self).$method(&other)
            }
        }

        impl<'a, F: $field_bound> $trait<&'a DensePolynomial<F>> for DensePolynomial<F> {
            type Output = DensePolynomial<F>;

            #[inline]
            fn $method(self, other: &'a DensePolynomial<F>) -> DensePolynomial<F> {
                (&self).$method(other)
            }
        }

        impl<'a, F: $field_bound> $trait<DensePolynomial<F>> for &'a DensePolynomial<F> {
            type Output = DensePolynomial<F>;

            #[inline]
            fn $method(self, other: DensePolynomial<F>) -> DensePolynomial<F> {
                self.$method(&other)
            }
        }
    };
}

impl<F: Field> Zero for DensePolynomial<F> {
    /// Returns the zero polynomial.
    fn zero() -> Self {
        Self { coeffs: Vec::new() }
    }

    /// Checks if the given polynomial is zero.
    fn is_zero(&self) -> bool {
        self.coeffs.is_empty() || self.coeffs.iter().all(|coeff| coeff.is_zero())
    }
}

impl_op!(Add, add, Field);
impl_op!(Sub, sub, Field);
impl_op!(Mul, mul, FftField);
impl_op!(Div, div, FftField);

#[cfg(test)]
mod tests {
    use crate::{polynomial::univariate::*, GeneralEvaluationDomain};
    use ark_ff::{Fp64, MontBackend, MontConfig};
    use ark_ff::{One, UniformRand};
    use ark_std::{rand::Rng, test_rng};
    use ark_test_curves::bls12_381::Fr;

    fn rand_sparse_poly<R: Rng>(degree: usize, rng: &mut R) -> SparsePolynomial<Fr> {
        // Initialize coeffs so that its guaranteed to have a x^{degree} term
        let mut coeffs = vec![(degree, Fr::rand(rng))];
        for i in 0..degree {
            if !rng.gen_bool(0.8) {
                coeffs.push((i, Fr::rand(rng)));
            }
        }
        SparsePolynomial::from_coefficients_vec(coeffs)
    }

    #[test]
    fn rand_dense_poly_degree() {
        #[derive(MontConfig)]
        #[modulus = "5"]
        #[generator = "2"]
        pub(crate) struct F5Config;

        let rng = &mut test_rng();
        pub(crate) type F5 = Fp64<MontBackend<F5Config, 1>>;

        // if the leading coefficient were uniformly sampled from all of F, this
        // test would fail with high probability ~99.9%
        for i in 1..=30 {
            assert_eq!(DensePolynomial::<F5>::rand(i, rng).degree(), i);
        }
    }

    #[test]
    fn double_polynomials_random() {
        let rng = &mut test_rng();
        for degree in 0..70 {
            let p = DensePolynomial::<Fr>::rand(degree, rng);
            let p_double = &p + &p;
            let p_quad = &p_double + &p_double;
            assert_eq!(&(&(&p + &p) + &p) + &p, p_quad);
        }
    }

    #[test]
    fn add_polynomials() {
        let rng = &mut test_rng();
        for a_degree in 0..70 {
            for b_degree in 0..70 {
                let p1 = DensePolynomial::<Fr>::rand(a_degree, rng);
                let p2 = DensePolynomial::<Fr>::rand(b_degree, rng);
                let res1 = &p1 + &p2;
                let res2 = &p2 + &p1;
                assert_eq!(res1, res2);
            }
        }
    }

    #[test]
    fn add_sparse_polynomials() {
        let rng = &mut test_rng();
        for a_degree in 0..70 {
            for b_degree in 0..70 {
                let p1 = DensePolynomial::<Fr>::rand(a_degree, rng);
                let p2 = rand_sparse_poly(b_degree, rng);
                let res = &p1 + &p2;
                assert_eq!(res, &p1 + &Into::<DensePolynomial<Fr>>::into(p2));
            }
        }
    }

    #[test]
    fn add_assign_sparse_polynomials() {
        let rng = &mut test_rng();
        for a_degree in 0..70 {
            for b_degree in 0..70 {
                let p1 = DensePolynomial::<Fr>::rand(a_degree, rng);
                let p2 = rand_sparse_poly(b_degree, rng);

                let mut res = p1.clone();
                res += &p2;
                assert_eq!(res, &p1 + &Into::<DensePolynomial<Fr>>::into(p2));
            }
        }
    }

    #[test]
    fn add_polynomials_with_mul() {
        let rng = &mut test_rng();
        for a_degree in 0..70 {
            for b_degree in 0..70 {
                let mut p1 = DensePolynomial::rand(a_degree, rng);
                let p2 = DensePolynomial::rand(b_degree, rng);
                let f = Fr::rand(rng);
                let f_p2 = DensePolynomial::from_coefficients_vec(
                    p2.coeffs.iter().map(|c| f * c).collect(),
                );
                let res2 = &f_p2 + &p1;
                p1 += (f, &p2);
                let res1 = p1;
                assert_eq!(res1, res2);
            }
        }
    }

    #[test]
    fn sub_polynomials() {
        let rng = &mut test_rng();
        for a_degree in 0..70 {
            for b_degree in 0..70 {
                let p1 = DensePolynomial::<Fr>::rand(a_degree, rng);
                let p2 = DensePolynomial::<Fr>::rand(b_degree, rng);
                let res1 = &p1 - &p2;
                let res2 = &p2 - &p1;
                assert_eq!(&res1 + &p2, p1);
                assert_eq!(res1, -res2);
            }
        }
    }

    #[test]
    fn sub_sparse_polynomials() {
        let rng = &mut test_rng();
        for a_degree in 0..70 {
            for b_degree in 0..70 {
                let p1 = DensePolynomial::<Fr>::rand(a_degree, rng);
                let p2 = rand_sparse_poly(b_degree, rng);
                let res = &p1 - &p2;
                assert_eq!(res, &p1 - &Into::<DensePolynomial<Fr>>::into(p2));
            }
        }
    }

    #[test]
    fn sub_assign_sparse_polynomials() {
        let rng = &mut test_rng();
        for a_degree in 0..70 {
            for b_degree in 0..70 {
                let p1 = DensePolynomial::<Fr>::rand(a_degree, rng);
                let p2 = rand_sparse_poly(b_degree, rng);

                let mut res = p1.clone();
                res -= &p2;
                assert_eq!(res, &p1 - &Into::<DensePolynomial<Fr>>::into(p2));
            }
        }
    }

    #[test]
    fn polynomial_additive_identity() {
        // Test adding polynomials with its negative equals 0
        let mut rng = test_rng();
        for degree in 0..70 {
            let poly = DensePolynomial::<Fr>::rand(degree, &mut rng);
            let neg = -poly.clone();
            let result = poly + neg;
            assert!(result.is_zero());
            assert_eq!(result.degree(), 0);

            // Test with SubAssign trait
            let poly = DensePolynomial::<Fr>::rand(degree, &mut rng);
            let mut result = poly.clone();
            result -= &poly;
            assert!(result.is_zero());
            assert_eq!(result.degree(), 0);
        }
    }

    #[test]
    fn divide_polynomials_fixed() {
        let dividend = DensePolynomial::from_coefficients_slice(&[
            "4".parse().unwrap(),
            "8".parse().unwrap(),
            "5".parse().unwrap(),
            "1".parse().unwrap(),
        ]);
        let divisor = DensePolynomial::from_coefficients_slice(&[Fr::one(), Fr::one()]); // Construct a monic linear polynomial.
        let result = &dividend / &divisor;
        let expected_result = DensePolynomial::from_coefficients_slice(&[
            "4".parse().unwrap(),
            "4".parse().unwrap(),
            "1".parse().unwrap(),
        ]);
        assert_eq!(expected_result, result);
    }

    #[test]
    fn divide_polynomials_random() {
        let rng = &mut test_rng();

        for a_degree in 0..50 {
            for b_degree in 0..50 {
                let dividend = DensePolynomial::<Fr>::rand(a_degree, rng);
                let divisor = DensePolynomial::<Fr>::rand(b_degree, rng);
                // Test the nlogn division
                if let Some(quotient) =
                    DenseOrSparsePolynomial::hensel_div(&(&dividend).into(), &(&divisor).into())
                {
                    let remainder = &dividend - &(&divisor * &quotient);
                    // ark_poly assumes that the 0 polynomial has degree 0 so we need to workaround that case
                    assert!(remainder.degree() < divisor.degree() || remainder.is_zero());
                }

                // Test the naive division
                if let Some((quotient, remainder)) =
                    DenseOrSparsePolynomial::naive_div(&(&dividend).into(), &(&divisor).into())
                {
                    assert!(remainder.degree() < divisor.degree() || remainder.is_zero());
                    assert_eq!(dividend, &(&divisor * &quotient) + &remainder);
                }
            }
        }
    }

    #[test]
    fn evaluate_polynomials() {
        let rng = &mut test_rng();
        for a_degree in 0..70 {
            let p = DensePolynomial::rand(a_degree, rng);
            let point: Fr = Fr::rand(rng);
            let mut total = Fr::zero();
            for (i, coeff) in p.coeffs.iter().enumerate() {
                total += &(point.pow([i as u64]) * coeff);
            }
            assert_eq!(p.evaluate(&point), total);
        }
    }

    #[test]
    fn mul_random_element() {
        let rng = &mut test_rng();
        for degree in 0..70 {
            let a = DensePolynomial::<Fr>::rand(degree, rng);
            let e = Fr::rand(rng);
            assert_eq!(
                &a * e,
                a.naive_mul(&DensePolynomial::from_coefficients_slice(&[e]))
            )
        }
    }

    #[test]
    fn mul_polynomials_random() {
        let rng = &mut test_rng();
        for a_degree in 0..70 {
            for b_degree in 0..70 {
                let a = DensePolynomial::<Fr>::rand(a_degree, rng);
                let b = DensePolynomial::<Fr>::rand(b_degree, rng);
                assert_eq!(&a * &b, a.naive_mul(&b))
            }
        }
    }

    #[test]
    fn mul_by_vanishing_poly() {
        let rng = &mut test_rng();
        for size in 1..10 {
            let domain = GeneralEvaluationDomain::new(1 << size).unwrap();
            for degree in 0..70 {
                let p = DensePolynomial::<Fr>::rand(degree, rng);
                let ans1 = p.mul_by_vanishing_poly(domain);
                let ans2 = &p * &domain.vanishing_polynomial().into();
                assert_eq!(ans1, ans2);
            }
        }
    }

    #[test]
    fn divide_by_vanishing_poly() {
        let rng = &mut test_rng();
        for size in 1..10 {
            let domain = GeneralEvaluationDomain::new(1 << size).unwrap();
            for degree in 0..12 {
                let p = DensePolynomial::<Fr>::rand(degree * 100, rng);
                let (quotient, remainder) = p.divide_by_vanishing_poly(domain);
                let p_recovered = quotient.mul_by_vanishing_poly(domain) + remainder;
                assert_eq!(p, p_recovered);
            }
        }
    }

    #[test]
    fn test_leading_zero() {
        let n = 10;
        let rand_poly = DensePolynomial::rand(n, &mut test_rng());
        let coefficients = rand_poly.coeffs.clone();
        let leading_coefficient: Fr = coefficients[n];

        let negative_leading_coefficient = -leading_coefficient;
        let inverse_leading_coefficient = leading_coefficient.inverse().unwrap();

        let mut inverse_coefficients = coefficients.clone();
        inverse_coefficients[n] = inverse_leading_coefficient;

        let mut negative_coefficients = coefficients;
        negative_coefficients[n] = negative_leading_coefficient;

        let negative_poly = DensePolynomial::<Fr>::from_coefficients_vec(negative_coefficients);
        let inverse_poly = DensePolynomial::<Fr>::from_coefficients_vec(inverse_coefficients);

        let x = &inverse_poly * &rand_poly;
        assert_eq!(x.degree(), 2 * n);
        assert!(!x.coeffs.last().unwrap().is_zero());

        let y = &negative_poly + &rand_poly;
        assert_eq!(y.degree(), n - 1);
        assert!(!y.coeffs.last().unwrap().is_zero());
    }

    #[test]
    fn evaluate_over_domain_test() {
        let rng = &mut ark_std::test_rng();
        let domain = crate::domain::Radix2EvaluationDomain::<Fr>::new(1 << 10).unwrap();
        let offset = Fr::GENERATOR;
        let coset = domain.get_coset(offset).unwrap();
        for _ in 0..100 {
            let poly = DensePolynomial::<Fr>::rand(1 << 11, rng);
            let evaluations = domain
                .elements()
                .map(|e| poly.evaluate(&e))
                .collect::<Vec<_>>();
            assert_eq!(evaluations, poly.evaluate_over_domain_by_ref(domain).evals);
            let evaluations = coset
                .elements()
                .map(|e| poly.evaluate(&e))
                .collect::<Vec<_>>();
            assert_eq!(evaluations, poly.evaluate_over_domain(coset).evals);
        }
        let zero = DensePolynomial::zero();
        let evaluations = domain
            .elements()
            .map(|e| zero.evaluate(&e))
            .collect::<Vec<_>>();
        assert_eq!(evaluations, zero.evaluate_over_domain(domain).evals);
    }

    use crate::Radix2EvaluationDomain;

    #[test]
    fn evaluate_over_domain_regression_test() {
        // See https://github.com/arkworks-rs/algebra/issues/745
        #[derive(MontConfig)]
        #[modulus = "18446744069414584321"]
        #[generator = "7"]
        struct FrConfig64;
        type F = Fp64<MontBackend<FrConfig64, 1>>;

        let degree = 17;
        let eval_domain_size = 16;

        let poly = DensePolynomial::from_coefficients_vec(vec![F::ONE; degree]);
        let domain = Radix2EvaluationDomain::new(eval_domain_size).unwrap();

        // Now we get a coset
        let offset = F::from(42u64);
        let domain = domain.get_coset(offset).unwrap();

        // This is the query points of the domain
        let query_points: Vec<_> = domain.elements().collect();

        let eval1 = poly.evaluate_over_domain_by_ref(domain).evals;
        let eval2 = query_points
            .iter()
            .map(|x| poly.evaluate(x))
            .collect::<Vec<_>>();

        assert_eq!(eval1, eval2);
    }

    #[test]
    fn test_add_assign_with_zero_self() {
        // Create a polynomial poly1 which is a zero polynomial
        let mut poly1 = DensePolynomial::<Fr> { coeffs: Vec::new() };

        // Create another polynomial poly2, which is: 2 + 3x (coefficients [2, 3])
        let poly2 = DensePolynomial {
            coeffs: vec![Fr::from(2), Fr::from(3)],
        };

        // Add poly2 to the zero polynomial
        // Since poly1 is zero, it should just take the coefficients of poly2.
        poly1 += (Fr::from(1), &poly2);

        // After addition, poly1 should be equal to poly2
        assert_eq!(poly1.coeffs, vec![Fr::from(2), Fr::from(3)]);
    }

    #[test]
    fn test_add_assign_with_zero_other() {
        // Create a polynomial poly1: 2 + 3x (coefficients [2, 3])
        let mut poly1 = DensePolynomial {
            coeffs: vec![Fr::from(2), Fr::from(3)],
        };

        // Create an empty polynomial poly2 (zero polynomial)
        let poly2 = DensePolynomial::<Fr> { coeffs: Vec::new() };

        // Add zero polynomial poly2 to poly1.
        // Since poly2 is zero, poly1 should remain unchanged.
        poly1 += (Fr::from(1), &poly2);

        // After addition, poly1 should still be [2, 3]
        assert_eq!(poly1.coeffs, vec![Fr::from(2), Fr::from(3)]);
    }

    #[test]
    fn test_add_assign_with_different_degrees() {
        // Create polynomial poly1: 1 + 2x + 3x^2 (coefficients [1, 2, 3])
        let mut poly1 = DensePolynomial {
            coeffs: vec![Fr::from(1), Fr::from(2), Fr::from(3)],
        };

        // Create another polynomial poly2: 4 + 5x (coefficients [4, 5])
        let poly2 = DensePolynomial {
            coeffs: vec![Fr::from(4), Fr::from(5)],
        };

        // Add poly2 to poly1.
        // poly1 is degree 2, poly2 is degree 1, so poly2 will be padded with a zero
        // to match the degree of poly1.
        poly1 += (Fr::from(1), &poly2);

        // After addition, the result should be:
        // 5 + 7x + 3x^2 (coefficients [5, 7, 3])
        assert_eq!(poly1.coeffs, vec![Fr::from(5), Fr::from(7), Fr::from(3)]);
    }

    #[test]
    fn test_add_assign_with_equal_degrees() {
        // Create polynomial poly1: 1 + 2x + 3x^2 (coefficients [1, 2, 3])
        let mut poly1 = DensePolynomial {
            coeffs: vec![Fr::from(1), Fr::from(2), Fr::from(3)],
        };

        // Create polynomial poly2: 4 + 5x + 6x^2 (coefficients [4, 5, 6])
        let poly2 = DensePolynomial {
            coeffs: vec![Fr::from(4), Fr::from(5), Fr::from(6)],
        };

        // Add poly2 to poly1.
        // Since both polynomials have the same degree, we can directly add corresponding terms.
        poly1 += (Fr::from(1), &poly2);

        // After addition, the result should be:
        // 5 + 7x + 9x^2 (coefficients [5, 7, 9])
        assert_eq!(poly1.coeffs, vec![Fr::from(5), Fr::from(7), Fr::from(9)]);
    }

    #[test]
    fn test_add_assign_with_smaller_degrees() {
        // Create polynomial poly1: 1 + 2x (degree 1)
        let mut poly1 = DensePolynomial {
            coeffs: vec![Fr::from(1), Fr::from(2)],
        };

        // Create polynomial poly2: 3 + 4x + 5x^2 (degree 2)
        let poly2 = DensePolynomial {
            coeffs: vec![Fr::from(3), Fr::from(4), Fr::from(5)],
        };

        // Add poly2 to poly1.
        // poly1 has degree 1, poly2 has degree 2. So poly1 must be padded with zero coefficients
        // for the higher degree terms to match poly2's degree.
        poly1 += (Fr::from(1), &poly2);

        // After addition, the result should be:
        // 4 + 6x + 5x^2 (coefficients [4, 6, 5])
        assert_eq!(poly1.coeffs, vec![Fr::from(4), Fr::from(6), Fr::from(5)]);
    }

    #[test]
    fn test_add_assign_mixed_with_zero_self() {
        // Create a zero DensePolynomial
        let mut poly1 = DensePolynomial::<Fr> { coeffs: Vec::new() };

        // Create a SparsePolynomial: 2 + 3x (coefficients [2, 3])
        let poly2 =
            SparsePolynomial::from_coefficients_slice(&[(0, Fr::from(2)), (1, Fr::from(3))]);

        // Add poly2 to the zero polynomial
        poly1 += &poly2;

        // After addition, the result should be 2 + 3x
        assert_eq!(poly1.coeffs, vec![Fr::from(2), Fr::from(3)]);
    }

    #[test]
    fn test_add_assign_mixed_with_zero_other() {
        // Create a DensePolynomial: 2 + 3x (coefficients [2, 3])
        let mut poly1 = DensePolynomial {
            coeffs: vec![Fr::from(2), Fr::from(3)],
        };

        // Create a zero SparsePolynomial
        let poly2 = SparsePolynomial::from_coefficients_slice(&[]);

        // Add poly2 to poly1
        poly1 += &poly2;

        // After addition, the result should still be 2 + 3x
        assert_eq!(poly1.coeffs, vec![Fr::from(2), Fr::from(3)]);
    }

    #[test]
    fn test_add_assign_mixed_with_different_degrees() {
        // Create a DensePolynomial: 1 + 2x + 3x^2 (coefficients [1, 2, 3])
        let mut poly1 = DensePolynomial {
            coeffs: vec![Fr::from(1), Fr::from(2), Fr::from(3)],
        };

        // Create a SparsePolynomial: 4 + 5x (coefficients [4, 5])
        let poly2 =
            SparsePolynomial::from_coefficients_slice(&[(0, Fr::from(4)), (1, Fr::from(5))]);

        // Add poly2 to poly1
        poly1 += &poly2;

        // After addition, the result should be 5 + 7x + 3x^2 (coefficients [5, 7, 3])
        assert_eq!(poly1.coeffs, vec![Fr::from(5), Fr::from(7), Fr::from(3)]);
    }

    #[test]
    fn test_add_assign_mixed_with_smaller_degree() {
        // Create a DensePolynomial: 1 + 2x (degree 1)
        let mut poly1 = DensePolynomial {
            coeffs: vec![Fr::from(1), Fr::from(2)],
        };

        // Create a SparsePolynomial: 3 + 4x + 5x^2 (degree 2)
        let poly2 = SparsePolynomial::from_coefficients_slice(&[
            (0, Fr::from(3)),
            (1, Fr::from(4)),
            (2, Fr::from(5)),
        ]);

        // Add poly2 to poly1
        poly1 += &poly2;

        // After addition, the result should be: 4 + 6x + 5x^2 (coefficients [4, 6, 5])
        assert_eq!(poly1.coeffs, vec![Fr::from(4), Fr::from(6), Fr::from(5)]);
    }

    #[test]
    fn test_add_assign_mixed_with_equal_degrees() {
        // Create a DensePolynomial: 1 + 2x + 3x^2 (coefficients [1, 2, 3])
        let mut poly1 = DensePolynomial {
            coeffs: vec![Fr::from(1), Fr::from(2), Fr::from(3)],
        };

        // Create a SparsePolynomial: 4 + 5x + 6x^2 (coefficients [4, 5, 6])
        let poly2 = SparsePolynomial::from_coefficients_slice(&[
            (0, Fr::from(4)),
            (1, Fr::from(5)),
            (2, Fr::from(6)),
        ]);

        // Add poly2 to poly1
        poly1 += &poly2;

        // After addition, the result should be 5 + 7x + 9x^2 (coefficients [5, 7, 9])
        assert_eq!(poly1.coeffs, vec![Fr::from(5), Fr::from(7), Fr::from(9)]);
    }

    #[test]
    fn test_add_assign_mixed_with_larger_degree() {
        // Create a DensePolynomial: 1 + 2x + 3x^2 + 4x^3 (degree 3)
        let mut poly1 = DensePolynomial {
            coeffs: vec![Fr::from(1), Fr::from(2), Fr::from(3), Fr::from(4)],
        };

        // Create a SparsePolynomial: 3 + 4x (degree 1)
        let poly2 =
            SparsePolynomial::from_coefficients_slice(&[(0, Fr::from(3)), (1, Fr::from(4))]);

        // Add poly2 to poly1
        poly1 += &poly2;

        // After addition, the result should be: 4 + 6x + 3x^2 + 4x^3 (coefficients [4, 6, 3, 4])
        assert_eq!(
            poly1.coeffs,
            vec![Fr::from(4), Fr::from(6), Fr::from(3), Fr::from(4)]
        );
    }

    #[test]
    fn test_truncate_leading_zeros_after_addition() {
        // Create a DensePolynomial: 1 + 2x + 3x^2 (coefficients [1, 2, 3])
        let mut poly1 = DensePolynomial {
            coeffs: vec![Fr::from(1), Fr::from(2), Fr::from(3)],
        };

        // Create a SparsePolynomial: -1 - 2x - 3x^2 (coefficients [-1, -2, -3])
        let poly2 = SparsePolynomial::from_coefficients_slice(&[
            (0, -Fr::from(1)),
            (1, -Fr::from(2)),
            (2, -Fr::from(3)),
        ]);

        // Add poly2 to poly1, which should result in a zero polynomial
        poly1 += &poly2;

        // The resulting polynomial should be zero, with an empty coefficient vector
        assert!(poly1.is_zero());
        assert_eq!(poly1.coeffs, vec![]);
    }

    #[test]
    fn test_truncate_leading_zeros_after_sparse_addition() {
        // Create a DensePolynomial with leading non-zero coefficients.
        let poly1 = DensePolynomial {
            coeffs: vec![Fr::from(3), Fr::from(2), Fr::from(1)],
        };

        // Create a SparsePolynomial to subtract the coefficients of poly1,
        // leaving trailing zeros after addition.
        let poly2 = SparsePolynomial::from_coefficients_slice(&[
            (0, -Fr::from(3)),
            (1, -Fr::from(2)),
            (2, -Fr::from(1)),
        ]);

        // Perform addition using the Add implementation.
        let result = &poly1 + &poly2;

        // Assert that the resulting polynomial is zero.
        assert!(result.is_zero(), "The resulting polynomial should be zero.");
        assert_eq!(result.coeffs, vec![], "Leading zeros were not truncated.");
    }

    #[test]
    fn test_dense_dense_add_assign_with_zero_self() {
        // Create a zero polynomial
        let mut poly1 = DensePolynomial { coeffs: Vec::new() };

        // Create a non-zero polynomial: 2 + 3x
        let poly2 = DensePolynomial {
            coeffs: vec![Fr::from(2), Fr::from(3)],
        };

        // Add the non-zero polynomial to the zero polynomial
        poly1 += &poly2;

        // Check that poly1 now equals poly2
        assert_eq!(poly1.coeffs, poly2.coeffs);
    }

    #[test]
    fn test_dense_dense_add_assign_with_zero_other() {
        // Create a non-zero polynomial: 2 + 3x
        let mut poly1 = DensePolynomial {
            coeffs: vec![Fr::from(2), Fr::from(3)],
        };

        // Create a zero polynomial
        let poly2 = DensePolynomial { coeffs: Vec::new() };

        // Add the zero polynomial to poly1
        poly1 += &poly2;

        // Check that poly1 remains unchanged
        assert_eq!(poly1.coeffs, vec![Fr::from(2), Fr::from(3)]);
    }

    #[test]
    fn test_dense_dense_add_assign_with_different_degrees() {
        // Create a polynomial: 1 + 2x + 3x^2
        let mut poly1 = DensePolynomial {
            coeffs: vec![Fr::from(1), Fr::from(2), Fr::from(3)],
        };

        // Create a smaller polynomial: 4 + 5x
        let poly2 = DensePolynomial {
            coeffs: vec![Fr::from(4), Fr::from(5)],
        };

        // Add the smaller polynomial to the larger one
        poly1 += &poly2;

        assert_eq!(poly1.coeffs, vec![Fr::from(5), Fr::from(7), Fr::from(3)]);
    }

    #[test]
    fn test_dense_dense_truncate_leading_zeros_after_addition() {
        // Create a first polynomial
        let mut poly1 = DensePolynomial {
            coeffs: vec![Fr::from(1), Fr::from(2)],
        };

        // Create another polynomial that will cancel out the first two terms
        let poly2 = DensePolynomial {
            coeffs: vec![-poly1.coeffs[0], -poly1.coeffs[1]],
        };

        // Add the two polynomials
        poly1 += &poly2;

        // Check that the resulting polynomial is zero (empty coefficients)
        assert!(poly1.is_zero());
        assert_eq!(poly1.coeffs, vec![]);
    }

    #[test]
    fn test_dense_dense_add_assign_with_equal_degrees() {
        // Create two polynomials with the same degree
        let mut poly1 = DensePolynomial {
            coeffs: vec![Fr::from(1), Fr::from(2), Fr::from(3)],
        };
        let poly2 = DensePolynomial {
            coeffs: vec![Fr::from(4), Fr::from(5), Fr::from(6)],
        };

        // Add the polynomials
        poly1 += &poly2;

        // Check the resulting coefficients
        assert_eq!(poly1.coeffs, vec![Fr::from(5), Fr::from(7), Fr::from(9)]);
    }

    #[test]
    fn test_dense_dense_add_assign_with_other_zero_truncates_leading_zeros() {
        use ark_test_curves::bls12_381::Fr;

        // Create a polynomial with leading zeros: 1 + 2x + 0x^2 + 0x^3
        let mut poly1 = DensePolynomial {
            coeffs: vec![Fr::from(1), Fr::from(2), Fr::from(0), Fr::from(0)],
        };

        // Create a zero polynomial
        let poly2 = DensePolynomial { coeffs: Vec::new() };

        // Add the zero polynomial to poly1
        poly1 += &poly2;

        // Check that the leading zeros are truncated
        assert_eq!(poly1.coeffs, vec![Fr::from(1), Fr::from(2)]);

        // Ensure the polynomial is not zero (as it has non-zero coefficients)
        assert!(!poly1.is_zero());
    }
}
