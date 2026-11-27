package lib

import "example.com/gobasic/internal/core"

type Svc struct{}

func NewSvc() *Svc {
	return &Svc{}
}

func (*Svc) Boot() int {
	return 7
}

func hook() int {
	return 2
}

var Handler = hook

func Compute() int {
	svc := NewSvc()
	return core.Mid() + svc.Boot()
}
