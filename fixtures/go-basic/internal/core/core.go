package core

func Leaf() int {
	return 1
}

func Mid() int {
	return Leaf()
}

func Helper() int {
	return Mid()
}
