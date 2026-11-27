package calc

import "testing"

func helper() int {
	return Add(1, 2)
}

func TestInternal(t *testing.T) {
	_ = helper()
}
