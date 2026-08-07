// Pure-gate fold check.
//
// Every deterministic, scalar-input pure gate that is NOT yet in the certified
// fold table (crates/wirescript/data/gate_semantics.json), each called with
// CONSTANT inputs. The table today only covers Compare*/Logical*, so with
// `@fold` on every gate below currently stays as a real gate (a fold barrier).
// Once a gate's input->output cases are probed into the table, the matching
// line here should fold to a constant literal instead — making this file a
// before/after fold regression check for those gates.
@fold

// ---- Math (float -> float) ----
out mSin: float   = sin(1.0)
out mCos: float   = cos(1.0)
out mTan: float   = tan(1.0)
out mAsin: float  = asin(0.5)
out mAcos: float  = acos(0.5)
out mAtan: float  = atan(1.0)
out mAtan2: float = atan2(1.0, 2.0)
out mSinh: float  = sinh(1.0)
out mCosh: float  = cosh(1.0)
out mTanh: float  = tanh(1.0)
out mAsinh: float = asinh(1.0)
out mAcosh: float = acosh(2.0)
out mAtanh: float = atanh(0.5)
out mExp: float   = exp(1.0)
out mLn: float    = ln(2.0)
out mLog: float   = log(8.0, 2.0)
out mSqrt: float  = sqrt(9.0)
out mPow: float   = pow(2.0, 10.0)
out mAbs: float   = abs(-4.0)
out mSign: float  = sign(-3.0)
out mClamp: float = clamp(5.0, 0.0, 1.0)
out mMin: float   = min(3.0, 7.0)
out mMax: float   = max(3.0, 7.0)
out mFmod: float  = fmod(7.0, 3.0)
out mD2R: float   = Deg2Rad(90.0)
out mR2D: float   = Rad2Deg(1.5708)
out mRound: float = round(2.6)
out mFloor: float = floor(2.9)
out mCeil: float  = ceil(2.1)

// ---- Bitwise / integer ----
out bAnd: int   = 12 & 10
out bOr: int    = 12 | 10
out bXor: int   = 12 ^ 10
out bNot: int   = ~12
out bShl: int   = 1 << 5
out bShr: int   = 64 >> 2
out bCount: int = BitCount(255)
out bNand: int  = BitNand(12, 10)
out bNor: int   = BitNor(12, 10)
out mModI: int  = 7 % 3

// ---- Vector / rotator / color decompose ----
out vSplit: float = Vec(1.0, 2.0, 3.0).SplitVec().x
out rEuler: float = Rotation(10.0, 20.0, 30.0).ToEuler().Yaw
out cLin: float   = Color(0.5, 0.6, 0.7).SplitColor().r
out cSrgb: int    = Color(0.5, 0.6, 0.7).ToSRGB().R

// ---- FormatDate: a NON-Expr pure gate (WireGraph_FormatDate). Deterministic
// given constant inputs when useUTC = true (local time would depend on the
// machine timezone). Foldable candidate. ----
out fDate: string = FormatDate(1700000000, "%Y-%m-%d %H:%M:%S", true).Output

// Omitted (no clean constant-input surface / stateful / removed / non-deterministic):
// RotToDir (removed), EdgeDetector (stateful), DeltaTime/GetUnixEpoch/ServerUptime
// (runtime time), Var/Timer/Tween/Buffer*/joints (stateful), Remap, NearlyEqual,
// IntegerToEnum, MathBlend, ShiftRightLogical, ConvertColor, String_Split.
