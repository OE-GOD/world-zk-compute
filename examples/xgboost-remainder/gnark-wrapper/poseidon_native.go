package main

import (
	"math/big"
)

// NativePoseidonSponge computes Poseidon over the BN254 scalar field (Fr).
// Used for computing witness values outside gnark circuits.
type NativePoseidonSponge struct {
	state [3]*big.Int
	rate  int
	p     *PoseidonParams
	mod   *big.Int // scalar field modulus (Fr)
}

// BN254 scalar field modulus
var frMod, _ = new(big.Int).SetString("21888242871839275222246405745257275088548364400416034343698204186575808495617", 10)

// NewNativePoseidonSponge creates a new native sponge over Fr.
func NewNativePoseidonSponge(params *PoseidonParams) *NativePoseidonSponge {
	return &NativePoseidonSponge{
		state: [3]*big.Int{new(big.Int).Lsh(big.NewInt(1), 64), big.NewInt(0), big.NewInt(0)},
		rate:  0,
		p:     params,
		mod:   frMod,
	}
}

// Absorb adds a field element.
func (s *NativePoseidonSponge) Absorb(value *big.Int) {
	if s.rate == 0 {
		s.state[1] = new(big.Int).Add(s.state[1], value)
		s.state[1].Mod(s.state[1], s.mod)
		s.rate = 1
	} else {
		s.state[2] = new(big.Int).Add(s.state[2], value)
		s.state[2].Mod(s.state[2], s.mod)
		s.rate = 0
		s.permute()
	}
}

// Squeeze returns a challenge.
func (s *NativePoseidonSponge) Squeeze() *big.Int {
	if s.rate == 0 {
		s.state[1] = new(big.Int).Add(s.state[1], big.NewInt(1))
		s.state[1].Mod(s.state[1], s.mod)
	} else {
		s.state[2] = new(big.Int).Add(s.state[2], big.NewInt(1))
		s.state[2].Mod(s.state[2], s.mod)
	}
	s.permute()
	s.rate = 0
	return new(big.Int).Set(s.state[1])
}

func (s *NativePoseidonSponge) permute() {
	q := s.mod
	p := s.p
	s0 := new(big.Int).Set(s.state[0])
	s1 := new(big.Int).Set(s.state[1])
	s2 := new(big.Int).Set(s.state[2])

	for r := 0; r < 65; r++ {
		// AddRoundConstants
		s0.Add(s0, p.RoundConstants[r][0])
		s0.Mod(s0, q)
		s1.Add(s1, p.RoundConstants[r][1])
		s1.Mod(s1, q)
		s2.Add(s2, p.RoundConstants[r][2])
		s2.Mod(s2, q)

		// S-box
		isFullRound := r < 4 || r >= 61
		if isFullRound {
			s0 = nativeSbox5(s0, q)
			s1 = nativeSbox5(s1, q)
			s2 = nativeSbox5(s2, q)
		} else {
			s0 = nativeSbox5(s0, q)
		}

		// MDS
		n0 := new(big.Int)
		n0.Mul(s0, p.MDS[0][0])
		n0.Mod(n0, q)
		tmp := new(big.Int).Mul(s1, p.MDS[0][1])
		tmp.Mod(tmp, q)
		n0.Add(n0, tmp)
		n0.Mod(n0, q)
		tmp.Mul(s2, p.MDS[0][2])
		tmp.Mod(tmp, q)
		n0.Add(n0, tmp)
		n0.Mod(n0, q)

		n1 := new(big.Int)
		n1.Mul(s0, p.MDS[1][0])
		n1.Mod(n1, q)
		tmp.Mul(s1, p.MDS[1][1])
		tmp.Mod(tmp, q)
		n1.Add(n1, tmp)
		n1.Mod(n1, q)
		tmp.Mul(s2, p.MDS[1][2])
		tmp.Mod(tmp, q)
		n1.Add(n1, tmp)
		n1.Mod(n1, q)

		n2 := new(big.Int)
		n2.Mul(s0, p.MDS[2][0])
		n2.Mod(n2, q)
		tmp.Mul(s1, p.MDS[2][1])
		tmp.Mod(tmp, q)
		n2.Add(n2, tmp)
		n2.Mod(n2, q)
		tmp.Mul(s2, p.MDS[2][2])
		tmp.Mod(tmp, q)
		n2.Add(n2, tmp)
		n2.Mod(n2, q)

		s0 = n0
		s1 = n1
		s2 = n2
	}

	s.state[0] = s0
	s.state[1] = s1
	s.state[2] = s2
}

func nativeSbox5(x, q *big.Int) *big.Int {
	x2 := new(big.Int).Mul(x, x)
	x2.Mod(x2, q)
	x4 := new(big.Int).Mul(x2, x2)
	x4.Mod(x4, q)
	x5 := new(big.Int).Mul(x, x4)
	x5.Mod(x5, q)
	return x5
}
