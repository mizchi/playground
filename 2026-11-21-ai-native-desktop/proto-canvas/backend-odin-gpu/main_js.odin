#+build js
// ブラウザ (js_wasm32) の入口: WebGPU の canvas (#gpu-screen) を表面にして、adapter と device を非同期に取る
package gpu

import "vendor:wgpu"

main :: proc() {
	state.ctx = context
	state.instance = wgpu.CreateInstance(nil)
	if state.instance == nil {
		state.failed = true
		return
	}
	if !offscreen {
		state.surface = wgpu.InstanceCreateSurface(state.instance, &wgpu.SurfaceDescriptor{
			nextInChain = &wgpu.SurfaceSourceCanvasHTMLSelector{sType = .SurfaceSourceCanvasHTMLSelector, selector = "#gpu-screen"},
		})
	}
	wgpu.InstanceRequestAdapter(state.instance, &{compatibleSurface = state.surface}, {callback = on_adapter})
}

on_adapter :: proc "c" (status: wgpu.RequestAdapterStatus, adapter: wgpu.Adapter, message: string, u1, u2: rawptr) {
	context = state.ctx
	if status != .Success || adapter == nil {
		state.failed = true
		return
	}
	state.adapter = adapter
	wgpu.AdapterRequestDevice(adapter, nil, {callback = on_device})
}
