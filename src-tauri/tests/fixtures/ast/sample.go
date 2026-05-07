package sample

import "fmt"

func StandaloneFunc(x int) int {
	return x + 1
}

type Counter struct {
	count int
}

func (c *Counter) Increment() {
	c.count++
}

func (c Counter) Value() int {
	return c.count
}

type Adder interface {
	Add(a, b int) int
}

var _ = fmt.Println
