package main

import (
	"bufio"
	"fmt"
	"net"
	"os"
	"strings"

	"github.com/la5nta/wl2k-go/fbb"
	"github.com/la5nta/wl2k-go/mailbox"
)

func line(r *bufio.Reader) string {
	s, _ := r.ReadString('\r')
	return strings.TrimSpace(s)
}

func main() {
	mode := os.Args[1]
	switch mode {
	case "server": // server ADDR MYCALL MBOX
		l, err := net.Listen("tcp", os.Args[2])
		if err != nil {
			panic(err)
		}
		fmt.Println("ready")
		conn, _ := l.Accept()
		fmt.Fprint(conn, "Callsign :\r")
		br := bufio.NewReader(conn)
		caller := strings.ToUpper(line(br))
		fmt.Fprint(conn, "Password :\r")
		line(br)
		s := fbb.NewSession(os.Args[3], caller, "", mailbox.NewDirHandler(os.Args[4], false))
		s.IsMaster(true)
		stats, err := s.Exchange(conn)
		fmt.Printf("go server: sent=%v received=%v err=%v\n", stats.Sent, stats.Received, err)
		if err != nil {
			os.Exit(1)
		}
	case "client": // client ADDR MYCALL TARGET MBOX
		conn, err := net.Dial("tcp", os.Args[2])
		if err != nil {
			panic(err)
		}
		br := bufio.NewReader(conn)
		for {
			l := strings.ToLower(line(br))
			if strings.HasPrefix(l, "callsign") {
				fmt.Fprintf(conn, "%s\r", os.Args[3])
			} else if strings.HasPrefix(l, "password") {
				fmt.Fprint(conn, "x\r")
				break
			}
		}
		s := fbb.NewSession(os.Args[3], os.Args[4], "", mailbox.NewDirHandler(os.Args[5], false))
		stats, err := s.Exchange(conn)
		fmt.Printf("go client: sent=%v received=%v err=%v\n", stats.Sent, stats.Received, err)
		if err != nil {
			os.Exit(1)
		}
	}
}
