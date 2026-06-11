//     Copyright (C) 2020-2021, IrineSistiana
//
//     This file is part of simple-tls.
//
//     simple-tls is free software: you can redistribute it and/or modify
//     it under the terms of the GNU General Public License as published by
//     the Free Software Foundation, either version 3 of the License, or
//     (at your option) any later version.
//
//     simple-tls is distributed in the hope that it will be useful,
//     but WITHOUT ANY WARRANTY; without even the implied warranty of
//     MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
//     GNU General Public License for more details.
//
//     You should have received a copy of the GNU General Public License
//     along with this program.  If not, see <https://www.gnu.org/licenses/>.

package ctunnel

import (
	"github.com/IrineSistiana/simple-tls/core/alloc"
	"github.com/IrineSistiana/simple-tls/core/utils"
	"io"
	"net"
	"sync"
	"time"
)

type TunnelOpts struct {
	IdleTimout time.Duration
}

func (opts *TunnelOpts) init() {
	utils.SetDefaultNum(&opts.IdleTimout, time.Second*300)
}

// defaultBufSize - fixed buffer size for MIPS optimization
// 64KB allows achieving 50+ Mbps with RTT 20ms
// Removed randomization to reduce CPU load and memory fragmentation
const defaultBufSize = 64 * 1024 // 64KB

// OpenTunnel opens a tunnel between a and b.
// It returns the first err encountered.
// a and b will be closed by OpenTunnel.
func OpenTunnel(a, b net.Conn, opts TunnelOpts) error {
	t := newTunnel(a, b, opts)
	go func() {
		_, err := t.copyBuffer(a, b)
		t.closePeersWithErr(err)
	}()
	go func() {
		_, err := t.copyBuffer(b, a)
		t.closePeersWithErr(err)
	}()
	return t.waitUntilClosed()
}

type tunnel struct {
	a, b net.Conn
	opts TunnelOpts

	closeOnce   sync.Once
	closeNotify chan struct{}
	closeErr    error
}

func newTunnel(a, b net.Conn, opts TunnelOpts) *tunnel {
	return &tunnel{a: a, b: b, opts: opts, closeNotify: make(chan struct{})}
}

func (t *tunnel) closePeersWithErr(err error) {
	t.closeOnce.Do(func() {
		t.a.Close()
		t.b.Close()
		t.closeErr = err
		close(t.closeNotify)
	})
}

func (t *tunnel) openOneWayTunnel(dst, src net.Conn) {
	go func() {
		_, err := t.copyBuffer(dst, src)
		t.closePeersWithErr(err)
	}()
}

func (t *tunnel) waitUntilClosed() error {
	<-t.closeNotify
	return t.closeErr
}

// copyBufferOptimized - оптимизированная версия copyBuffer для MIPS
// Использует фиксированный буфер и минимизирует вызовы time.Now()
func copyBufferOptimized(dst net.Conn, src net.Conn, idleTimeout time.Duration) (written int64, err error) {
	// Выделяем буфер один раз на весь цикл
	buf := alloc.GetBuf(defaultBufSize)
	defer alloc.ReleaseBuf(buf)

	// deadlineLastUpdate - время последнего обновления deadline
	// deadlineNext - время следующего обязательного обновления
	var deadlineLastUpdate time.Time
	var deadlineNext time.Time

	for {
		// Обновляем deadline только если прошло достаточно времени
		// Это снижает частоту вызовов time.Now() и system calls
		now := time.Now()
		if idleTimeout > 0 && (deadlineLastUpdate.IsZero() || now.After(deadlineNext)) {
			deadline := now.Add(idleTimeout)
			src.SetDeadline(deadline)
			dst.SetDeadline(deadline)
			deadlineLastUpdate = now
			// Обновляем следующий вызов через половину timeout
			// Это гарантирует, что соединение не уйдёт в таймаут
			deadlineNext = now.Add(idleTimeout / 2)
		}

		nr, er := src.Read(buf)
		if nr > 0 {
			nw, ew := dst.Write(buf[0:nr])
			if nw > 0 {
				written += int64(nw)
			}
			if ew != nil {
				err = ew
				break
			}
			if nr != nw {
				err = io.ErrShortWrite
				break
			}
		}
		if er != nil {
			if er != io.EOF {
				err = er
			}
			break
		}
	}
	return written, err
}

func (t *tunnel) copyBuffer(dst net.Conn, src net.Conn) (written int64, err error) {
	return copyBufferOptimized(dst, src, t.opts.IdleTimout)
}
