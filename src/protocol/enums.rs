//! Enumerations used by the Fluke Connect binary reading record.
//!
//! Every enumeration carries an `Unknown(u8)` variant so that codes emitted
//! by devices newer than this crate decode without error and round-trip
//! back to their wire value.

/// Generates a `#[repr(u8)]`-style enumeration with lossless conversion from
/// and to the raw wire code.
macro_rules! wire_enum {
    (
        $(#[$meta:meta])*
        $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident = $code:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
        #[non_exhaustive]
        pub enum $name {
            $( $(#[$vmeta])* $variant, )+
            /// A code this crate does not know about yet.
            Unknown(u8),
        }

        impl $name {
            /// Decodes a raw wire code.
            #[must_use]
            pub const fn from_raw(code: u8) -> Self {
                match code {
                    $( $code => Self::$variant, )+
                    other => Self::Unknown(other),
                }
            }

            /// Returns the raw wire code.
            #[must_use]
            pub const fn raw(self) -> u8 {
                match self {
                    $( Self::$variant => $code, )+
                    Self::Unknown(other) => other,
                }
            }
        }

        impl From<u8> for $name {
            fn from(code: u8) -> Self {
                Self::from_raw(code)
            }
        }

        impl From<$name> for u8 {
            fn from(value: $name) -> Self {
                value.raw()
            }
        }
    };
}

wire_enum! {
    /// Display state of a reading.
    ///
    /// Anything other than [`ReadingState::Normal`] means the numeric value
    /// should not be trusted as a measurement.
    ReadingState {
        /// A valid measurement is being displayed.
        Normal = 0,
        /// The display is blank (for example while auto-ranging).
        Blank = 1,
        /// The reading is inactive.
        Inactive = 2,
        /// No valid reading is available (for example right after a function change).
        Invalid = 3,
        /// Over range (`OL` on the meter display).
        OverRange = 4,
        /// Analog-to-digital converter overload.
        OverloadA2d = 5,
        /// Open thermocouple.
        OpenThermocouple = 6,
        /// A capacitor is being discharged.
        Discharge = 7,
        /// Test leads are in the wrong jacks.
        Leads = 8,
        /// The value is greater than the displayed number.
        GreaterThan = 9,
        /// A phase is missing.
        MissingPhase = 10,
        /// A meter error.
        Error = 11,
        /// The value is less than the displayed number.
        LessThan = 12,
        /// No reading slot is populated.
        Empty = 13,
    }
}

wire_enum! {
    /// SI magnitude prefix applied to the displayed value.
    Magnitude {
        /// No prefix (10^0).
        None = 0,
        /// Giga (10^9).
        Giga = 1,
        /// Mega (10^6).
        Mega = 2,
        /// Kilo (10^3).
        Kilo = 3,
        /// Milli (10^-3).
        Milli = 4,
        /// Micro (10^-6).
        Micro = 5,
        /// Nano (10^-9).
        Nano = 6,
        /// Pico (10^-12).
        Pico = 7,
    }
}

impl Magnitude {
    /// Decimal exponent of this prefix, or `0` for an unknown code.
    #[must_use]
    pub const fn exponent(self) -> i8 {
        match self {
            Self::None | Self::Unknown(_) => 0,
            Self::Giga => 9,
            Self::Mega => 6,
            Self::Kilo => 3,
            Self::Milli => -3,
            Self::Micro => -6,
            Self::Nano => -9,
            Self::Pico => -12,
        }
    }

    /// SI prefix symbol, or an empty string for none/unknown.
    #[must_use]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::None | Self::Unknown(_) => "",
            Self::Giga => "G",
            Self::Mega => "M",
            Self::Kilo => "k",
            Self::Milli => "m",
            Self::Micro => "µ",
            Self::Nano => "n",
            Self::Pico => "p",
        }
    }
}

wire_enum! {
    /// Unit of measure of a reading.
    Unit {
        /// No unit.
        None = 0,
        /// Volts AC.
        VoltsAc = 1,
        /// Volts DC.
        VoltsDc = 2,
        /// Amperes AC.
        AmpsAc = 3,
        /// Amperes DC.
        AmpsDc = 4,
        /// Hertz.
        Hertz = 5,
        /// Percent relative humidity.
        PercentRh = 6,
        /// Degrees Celsius.
        Celsius = 7,
        /// Degrees Fahrenheit.
        Fahrenheit = 8,
        /// Degrees Rankine.
        Rankine = 9,
        /// Kelvin.
        Kelvin = 10,
        /// Ohms.
        Ohms = 11,
        /// Siemens.
        Siemens = 12,
        /// Duty cycle percent.
        PercentDuty = 13,
        /// Seconds.
        Seconds = 14,
        /// Farads.
        Farads = 15,
        /// Decibels.
        Decibels = 16,
        /// Decibel-milliwatts.
        DecibelMilliwatts = 17,
        /// Watts.
        Watts = 18,
        /// Joules.
        Joules = 19,
        /// Henries.
        Henries = 20,
        /// Pounds per square inch.
        Psi = 21,
        /// Metres of mercury.
        MetresHg = 22,
        /// Inches of mercury.
        InchesHg = 23,
        /// Feet of water.
        FeetH2o = 24,
        /// Metres of water.
        MetresH2o = 25,
        /// Inches of water.
        InchesH2o = 26,
        /// Inches of water at 60 °F.
        InchesH2o60F = 27,
        /// Bar.
        Bar = 28,
        /// Pascals.
        Pascals = 29,
        /// Grams per square centimetre.
        GramsPerCm2 = 30,
        /// Decibel-volts.
        DecibelVolts = 31,
        /// Crest factor (dimensionless).
        CrestFactor = 32,
        /// Volts AC plus DC.
        VoltsAcPlusDc = 33,
        /// Amperes AC plus DC.
        AmpsAcPlusDc = 34,
        /// Percent.
        Percent = 35,
        /// Volts AC per hertz.
        VoltsAcPerHertz = 36,
        /// Acceleration in g.
        AccelerationG = 37,
        /// Acceleration in metres per second squared.
        AccelerationMps2 = 38,
        /// Velocity in inches per second.
        VelocityIps = 39,
        /// Velocity in millimetres per second.
        VelocityMmps = 40,
        /// Displacement in mils.
        DisplacementMils = 41,
        /// Displacement in microns.
        DisplacementMicrons = 42,
        /// The device reported an unknown unit.
        DeviceUnknown = 43,
        /// Tera-ohms.
        TeraOhms = 44,
    }
}

impl Unit {
    /// Short display symbol for the unit.
    #[must_use]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::None | Self::Unknown(_) => "",
            Self::VoltsAc => "V AC",
            Self::VoltsDc => "V DC",
            Self::AmpsAc => "A AC",
            Self::AmpsDc => "A DC",
            Self::Hertz => "Hz",
            Self::PercentRh => "%RH",
            Self::Celsius => "°C",
            Self::Fahrenheit => "°F",
            Self::Rankine => "°R",
            Self::Kelvin => "K",
            Self::Ohms => "Ω",
            Self::Siemens => "S",
            Self::PercentDuty | Self::Percent => "%",
            Self::Seconds => "s",
            Self::Farads => "F",
            Self::Decibels => "dB",
            Self::DecibelMilliwatts => "dBm",
            Self::Watts => "W",
            Self::Joules => "J",
            Self::Henries => "H",
            Self::Psi => "psi",
            Self::MetresHg => "mHg",
            Self::InchesHg => "inHg",
            Self::FeetH2o => "ftH2O",
            Self::MetresH2o => "mH2O",
            Self::InchesH2o => "inH2O",
            Self::InchesH2o60F => "inH2O@60°F",
            Self::Bar => "bar",
            Self::Pascals => "Pa",
            Self::GramsPerCm2 => "g/cm²",
            Self::DecibelVolts => "dBV",
            Self::CrestFactor => "CF",
            Self::VoltsAcPlusDc => "V AC+DC",
            Self::AmpsAcPlusDc => "A AC+DC",
            Self::VoltsAcPerHertz => "V/Hz",
            Self::AccelerationG => "g",
            Self::AccelerationMps2 => "m/s²",
            Self::VelocityIps => "in/s",
            Self::VelocityMmps => "mm/s",
            Self::DisplacementMils => "mil",
            Self::DisplacementMicrons => "µm",
            Self::DeviceUnknown => "?",
            Self::TeraOhms => "TΩ",
        }
    }

    /// Decimal exponent that converts a value in this unit to its SI base
    /// unit (for example tera-ohms to ohms). Zero for units that already are
    /// a base unit or have no SI base.
    #[must_use]
    pub const fn base_exponent(self) -> i8 {
        match self {
            Self::TeraOhms => 12,
            Self::VelocityMmps => -3,
            Self::DisplacementMicrons => -6,
            Self::None
            | Self::VoltsAc
            | Self::VoltsDc
            | Self::AmpsAc
            | Self::AmpsDc
            | Self::Hertz
            | Self::PercentRh
            | Self::Celsius
            | Self::Fahrenheit
            | Self::Rankine
            | Self::Kelvin
            | Self::Ohms
            | Self::Siemens
            | Self::PercentDuty
            | Self::Seconds
            | Self::Farads
            | Self::Decibels
            | Self::DecibelMilliwatts
            | Self::Watts
            | Self::Joules
            | Self::Henries
            | Self::Psi
            | Self::MetresHg
            | Self::InchesHg
            | Self::FeetH2o
            | Self::MetresH2o
            | Self::InchesH2o
            | Self::InchesH2o60F
            | Self::Bar
            | Self::Pascals
            | Self::GramsPerCm2
            | Self::DecibelVolts
            | Self::CrestFactor
            | Self::VoltsAcPlusDc
            | Self::AmpsAcPlusDc
            | Self::Percent
            | Self::VoltsAcPerHertz
            | Self::AccelerationG
            | Self::AccelerationMps2
            | Self::VelocityIps
            | Self::DisplacementMils
            | Self::DeviceUnknown
            | Self::Unknown(_) => 0,
        }
    }
}

wire_enum! {
    /// Measurement function the meter is set to.
    ///
    /// Codes above 51 belong to insulation testers, installation testers and
    /// pressure modules in the Fluke Connect family and are included for
    /// completeness.
    Function {
        /// No function.
        None = 0,
        /// Millivolts AC.
        MilliVoltsAc = 1,
        /// Volts AC.
        VoltsAc = 2,
        /// Volts AC plus DC.
        VoltsAcPlusDc = 3,
        /// Millivolts AC, average responding.
        MilliVoltsAcAverage = 4,
        /// Volts AC, average responding.
        VoltsAcAverage = 5,
        /// Volts AC plus DC, average responding.
        VoltsAcAveragePlusDc = 6,
        /// Volts AC, low impedance.
        VoltsAcLowZ = 7,
        /// Millivolts AC, low-pass filtered.
        MilliVoltsAcLowPass = 8,
        /// Volts AC, low-pass filtered.
        VoltsAcLowPass = 9,
        /// Microvolts DC.
        MicroVoltsDc = 10,
        /// Millivolts DC.
        MilliVoltsDc = 11,
        /// Volts DC.
        VoltsDc = 12,
        /// Milliamps AC.
        MilliAmpsAc = 13,
        /// Amps AC.
        AmpsAc = 14,
        /// Amps AC plus DC.
        AmpsAcPlusDc = 15,
        /// Milliamps AC, average responding.
        MilliAmpsAcAverage = 16,
        /// Amps AC, average responding.
        AmpsAcAverage = 17,
        /// Amps AC plus DC, average responding.
        AmpsAcAveragePlusDc = 18,
        /// Microamps DC.
        MicroAmpsDc = 19,
        /// Milliamps DC.
        MilliAmpsDc = 20,
        /// Amps DC.
        AmpsDc = 21,
        /// Frequency while measuring millivolts AC.
        HertzMilliVoltsAc = 22,
        /// Frequency while measuring volts AC.
        HertzVoltsAc = 23,
        /// Frequency while measuring millivolts AC, low-pass filtered.
        HertzMilliVoltsAcLowPass = 24,
        /// Frequency while measuring volts AC, low-pass filtered.
        HertzVoltsAcLowPass = 25,
        /// Frequency while measuring millivolts DC.
        HertzMilliVoltsDc = 26,
        /// Frequency while measuring volts DC.
        HertzVoltsDc = 27,
        /// Frequency while measuring microamps AC.
        HertzMicroAmpsAc = 28,
        /// Frequency while measuring milliamps AC.
        HertzMilliAmpsAc = 29,
        /// Frequency while measuring amps AC.
        HertzAmpsAc = 30,
        /// Frequency while measuring microamps DC.
        HertzMicroAmpsDc = 31,
        /// Frequency while measuring milliamps DC.
        HertzMilliAmpsDc = 32,
        /// Frequency while measuring amps DC.
        HertzAmpsDc = 33,
        /// Temperature.
        Temperature = 34,
        /// Temperature in degrees Fahrenheit.
        Fahrenheit = 35,
        /// Temperature in degrees Celsius.
        Celsius = 36,
        /// Temperature in degrees Rankine.
        Rankine = 37,
        /// Temperature in kelvin.
        Kelvin = 38,
        /// Continuity.
        Continuity = 39,
        /// Resistance.
        Resistance = 40,
        /// Conductance.
        Conductance = 41,
        /// Low-ohms resistance.
        LowOhms = 42,
        /// Phase angle in degrees.
        DegreesPhase = 43,
        /// Inrush current, AC.
        AmpsAcInrush = 44,
        /// Capacitance.
        Capacitance = 45,
        /// Diode test.
        Diode = 46,
        /// Volts AC per hertz.
        VoltsAcPerHertz = 47,
        /// Millivolts AC and DC.
        MilliVoltsAcDc = 48,
        /// Milliamps AC and DC.
        MilliAmpsAcDc = 49,
        /// Microamps AC.
        MicroAmpsAc = 50,
        /// Microamps AC and DC.
        MicroAmpsAcDc = 51,
        /// Insulation tester: test voltage DC.
        TestVoltsDc = 52,
        /// Installation tester: fault voltage AC.
        FaultVoltsAc = 53,
        /// Installation tester: touch voltage limit.
        TouchVoltsAcLimit = 54,
        /// Insulation resistance.
        Insulation = 55,
        /// Continuity with positive polarity.
        ContinuityPositive = 56,
        /// Continuity with negative polarity.
        ContinuityNegative = 57,
        /// Continuity, averaged polarity.
        ContinuityAverage = 58,
        /// Loop impedance, tripping.
        LoopTrip = 59,
        /// Loop impedance, non-tripping.
        LoopNoTrip = 60,
        /// Loop impedance.
        LoopImpedance = 61,
        /// Maximum loop impedance.
        LoopImpedanceMax = 62,
        /// Line impedance.
        LineImpedance = 63,
        /// Maximum line impedance.
        LineImpedanceMax = 64,
        /// RCD trip time.
        RcdTime = 65,
        /// RCD trip time, automatic sequence.
        RcdTimeAuto = 66,
        /// RCD ramp test.
        RcdRamp = 67,
        /// Phase rotation.
        PhaseRotation = 68,
        /// Earth resistance.
        EarthResistance = 69,
        /// Earth fault current.
        EarthFaultCurrent = 70,
        /// Prospective short-circuit current.
        ShortCircuitCurrent = 71,
        /// Installation tester automatic test.
        MftAutoTest = 72,
        /// Insulation spot test, voltage detect phase.
        InsulationSpotVoltsDetect = 73,
        /// Insulation spot test.
        InsulationSpotTest = 74,
        /// Polarization index test, voltage detect phase.
        InsulationPiVoltsDetect = 75,
        /// Polarization index test.
        InsulationPiTest = 76,
        /// Polarization index result.
        InsulationPiResult = 77,
        /// Dielectric absorption ratio test, voltage detect phase.
        InsulationDarVoltsDetect = 78,
        /// Dielectric absorption ratio test.
        InsulationDarTest = 79,
        /// Dielectric absorption ratio result.
        InsulationDarResult = 80,
        /// Fluke 1555 ramp test, voltage detect phase.
        RampVoltsDetect1555 = 81,
        /// Fluke 1555 ramp test.
        RampVoltsTest1555 = 82,
        /// Fluke 1555 DAR (China) test, voltage detect phase.
        DarChinaVoltsDetect1555 = 83,
        /// Fluke 1555 DAR (China) test.
        DarChinaVoltsTest1555 = 84,
        /// Fluke 1555 DAR (China) result.
        DarChinaResult1555 = 85,
        /// Pressure.
        Pressure = 86,
        /// Fluke 37x clamp field sense.
        FieldSense37x = 87,
    }
}

wire_enum! {
    /// Range decade hint carried alongside the range number.
    Decade {
        /// No decade information.
        None = 0,
        /// Tens.
        Tens = 1,
        /// Hundreds.
        Hundreds = 2,
        /// Thousands.
        Thousands = 3,
        /// Thousandths.
        Milli = 4,
        /// Hundredths.
        Centi = 5,
        /// Tenths.
        Deci = 6,
    }
}

wire_enum! {
    /// Qualifier attached to a reading.
    Attribute {
        /// No attribute.
        None = 0,
        /// Open circuit.
        OpenCircuit = 1,
        /// Short circuit.
        ShortCircuit = 2,
        /// Intermittent (glitch) circuit.
        GlitchCircuit = 3,
        /// Diode tested good.
        GoodDiode = 4,
        /// Negative edge.
        NegativeEdge = 5,
        /// Positive edge.
        PositiveEdge = 6,
        /// High current.
        HighCurrent = 7,
        /// Hazardous voltage indicator.
        HazardousVoltage = 8,
        /// Low ohms.
        LowOhms = 9,
        /// Open circuit with glitch.
        OpenGlitchCircuit = 10,
        /// Short circuit with glitch.
        ShortGlitchCircuit = 11,
        /// Peak value.
        Peak = 12,
        /// Sourced value.
        Sourced = 13,
        /// Simulated value.
        Simulated = 14,
        /// Noise present.
        Noise = 15,
        /// Breakdown detected.
        Breakdown = 16,
    }
}

#[cfg(test)]
mod tests {
    use super::{Attribute, Decade, Function, Magnitude, ReadingState, Unit};

    #[test]
    fn known_codes_round_trip() {
        for code in 0..=u8::MAX {
            assert_eq!(ReadingState::from_raw(code).raw(), code);
            assert_eq!(Magnitude::from_raw(code).raw(), code);
            assert_eq!(Unit::from_raw(code).raw(), code);
            assert_eq!(Function::from_raw(code).raw(), code);
            assert_eq!(Decade::from_raw(code).raw(), code);
            assert_eq!(Attribute::from_raw(code).raw(), code);
        }
    }

    #[test]
    fn unknown_codes_are_preserved() {
        assert_eq!(Unit::from_raw(200), Unit::Unknown(200));
        assert_eq!(u8::from(Function::Unknown(99)), 99);
    }

    #[test]
    fn magnitude_exponents() {
        assert_eq!(Magnitude::Milli.exponent(), -3);
        assert_eq!(Magnitude::Kilo.symbol(), "k");
        assert_eq!(Magnitude::Unknown(7).exponent(), 0);
    }
}
