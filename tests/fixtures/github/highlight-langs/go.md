```go
package main

import (
	"fmt"
	"net/http"
)

// Server serves.
type Server struct {
	Addr string `json:"addr"`
	mu   sync.Mutex
}

type Handler interface {
	ServeHTTP(w http.ResponseWriter, r *http.Request)
}

func (s *Server) Start(ctx context.Context, port int) (err error) {
	defer s.mu.Unlock()
	ch := make(chan int, 10)
	go func() { ch <- 42 }()
	select {
	case v := <-ch:
		fmt.Printf("got %d\n", v)
	default:
	}
	var x float64 = 3.14
	if x > 0 && port != 0 {
		return nil
	}
	return fmt.Errorf("bad: %w", err)
}

func main() { s := &Server{Addr: ":8080"}; _ = s.Start(nil, 1) }
```
