#pragma once
// Vehicle shapes from probes (contributed by GTAxSkate): a vehicle model with
// no extracted bound (most DLC cars) is surveyed once with ray probes down its
// own up axis, so Skate gets a roof, hood and trunk to ride and a gap under
// it to hippie jump, instead of the bounding box (vehicle_shapes.rs).

class RuntimeBridge;

namespace vehiclesurvey {

// While skating: surveys at most one new model per call among the vehicles
// within `radius` m of the player.
void Tick(RuntimeBridge& runtime, float radius, void (*log)(const char*));

} // namespace vehiclesurvey
