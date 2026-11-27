package main

import (
	"math/big"
	"strings"
	"testing"
)

func TestParseBigIntReadsHexAndDecimal(t *testing.T) {
	cases := map[string]int64{
		"0x0":   0,
		"0x1f":  31,
		"0x01F": 31,
		"31":    31,
		"0":     0,
	}
	for in, want := range cases {
		got, err := parseBigInt(in)
		if err != nil {
			t.Fatalf("parseBigInt(%q): unexpected error %v", in, err)
		}
		if got.Cmp(big.NewInt(want)) != 0 {
			t.Fatalf("parseBigInt(%q) = %v, want %d", in, got, want)
		}
	}
}

func TestParseBigIntRejectsMalformedNumbers(t *testing.T) {
	for _, in := range []string{"", "0x", "0xzz", "12a", "-1", "0x-1", " 1"} {
		if v, err := parseBigInt(in); err == nil {
			t.Fatalf("parseBigInt(%q) = %v, want an error", in, v)
		}
	}
}

func TestBigIntParserKeepsFirstError(t *testing.T) {
	p := &bigIntParser{}
	if v := p.parse("0x2"); v.Cmp(big.NewInt(2)) != 0 || p.err != nil {
		t.Fatalf("good value: got %v, err %v", v, p.err)
	}
	p.parse("bad1")
	p.parse("bad2")
	if p.err == nil {
		t.Fatal("parser did not record the bad value")
	}
	if want := `"bad1"`; !strings.Contains(p.err.Error(), want) {
		t.Fatalf("error %q does not name the first bad value %s", p.err, want)
	}
}
