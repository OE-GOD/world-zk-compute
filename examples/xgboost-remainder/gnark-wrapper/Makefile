BINARY := gnark-wrapper
MODULE := github.com/worldcoin/world-zk-compute/examples/xgboost-remainder/gnark-wrapper

.PHONY: build build-linux build-darwin-arm64 test clean

build:
	go build -o $(BINARY) .

build-linux:
	GOOS=linux GOARCH=amd64 go build -o $(BINARY)-linux-amd64 .

build-darwin-arm64:
	GOOS=darwin GOARCH=arm64 go build -o $(BINARY)-darwin-arm64 .

test:
	go test -v ./...

clean:
	rm -f $(BINARY) $(BINARY)-linux-amd64 $(BINARY)-darwin-arm64
