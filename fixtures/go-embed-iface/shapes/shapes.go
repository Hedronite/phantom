package shapes

type Shape interface {
	Area() int
}

type Base struct{}

func (Base) Hello() int {
	return 10
}

type Outer struct {
	Base
}

type Rect struct{}

func (*Rect) Area() int {
	return 4
}

func (*Rect) UniqueStep() int {
	return 5
}

var _ Shape = (*Rect)(nil)

type Circle struct{}

func (Circle) Area() int {
	return 9
}

func Use(o Outer, s Shape, unk interface{ UniqueStep() int }) int {
	return o.Hello() + s.Area() + unk.UniqueStep()
}
