package artifact

import _ "embed"

//go:generate go run ../../cmd/update-wasm

//go:embed exvlan_core.wasm
var core []byte

func Core() []byte {
	return core
}
