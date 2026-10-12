#include "skate_map.h"
#include "natives.h"
#include <windows.h>
#include <vector>

namespace skatemap {
namespace {
using namespace gta;

// GTA's skate spots, found in its map data: the parks' collision (vb_34_skate,
// ...) and where its ymaps place the prop_skate_* ramps.
struct Spot {
    const char* name;
    float x, y;
};
constexpr Spot kSpots[] = {
    {"Vespucci Beach Skatepark", -1374.0f, -1396.0f},
    {"Hawick Skate Spot", 280.0f, -206.0f},
    {"Little Seoul Skate Spot", -950.0f, -786.0f},
    {"Strawberry Halfpipe", -266.0f, -1571.0f},
    {"La Mesa Rails", 718.0f, -1231.0f},
};
constexpr int kSpotSprite = 1;  // the plain circle, short range like GTA's activities
constexpr int kSpotColour = 5;  // yellow
constexpr int kWaypointSprite = 8;

std::vector<int> g_spots;
bool g_route = false;
float g_routeX = 0.0f, g_routeY = 0.0f;
DWORD g_nextCheck = 0;

void SetRoute(bool on, float x = 0.0f, float y = 0.0f, float z = 0.0f, int colour = 0) {
    Call<void>(CLEAR_GPS_MULTI_ROUTE);
    if (on) {
        Call<void>(START_GPS_MULTI_ROUTE, colour, TRUE, TRUE);
        Call<void>(ADD_POINT_TO_GPS_MULTI_ROUTE, x, y, z);
        Call<void>(SET_GPS_MULTI_ROUTE_RENDER, TRUE);
    }
    g_route = on;
    g_routeX = x;
    g_routeY = y;
}

void Spots(bool on) {
    bool all = !g_spots.empty();
    for (const int b : g_spots) all = all && Call<BOOL>(DOES_BLIP_EXIST, b);
    if (on && all) return;
    for (const int b : g_spots)
        if (Call<BOOL>(DOES_BLIP_EXIST, b)) {
            int handle = b;
            Call<void>(REMOVE_BLIP, &handle);
        }
    g_spots.clear();
    if (!on) return;
    for (const Spot& s : kSpots) {
        const int b = Call<int>(ADD_BLIP_FOR_COORD, s.x, s.y, 0.0f);
        if (!b) continue;
        Call<void>(SET_BLIP_SPRITE, b, kSpotSprite);
        Call<void>(SET_BLIP_COLOUR, b, kSpotColour);
        Call<void>(SET_BLIP_AS_SHORT_RANGE, b, TRUE);
        Call<void>(BEGIN_TEXT_COMMAND_SET_BLIP_NAME, "STRING");
        Call<void>(ADD_TEXT_COMPONENT_SUBSTRING_PLAYER_NAME, s.name);
        Call<void>(END_TEXT_COMMAND_SET_BLIP_NAME, b);
        g_spots.push_back(b);
    }
}

} // namespace

void Tick(bool skating, bool gps, bool spots) {
    const DWORD now = GetTickCount();
    if (static_cast<LONG>(now - g_nextCheck) < 0) return;
    g_nextCheck = now + 1000;  // once a second is plenty for both
    Spots(spots);
    // GTA draws the route to the waypoint only on foot or in a vehicle; in
    // Skate mode the player is neither, so it is drawn here (as a vehicle's).
    if (gps && skating && Call<BOOL>(IS_WAYPOINT_ACTIVE)) {
        const int blip = Call<int>(GET_FIRST_BLIP_INFO_ID, kWaypointSprite);
        if (blip) {
            const Vector3 at = Call<Vector3>(GET_BLIP_INFO_ID_COORD, blip);
            const float dx = at.x - g_routeX, dy = at.y - g_routeY;
            if (!g_route || dx * dx + dy * dy > 4.0f)
                SetRoute(true, at.x, at.y, at.z, Call<int>(GET_BLIP_HUD_COLOUR, blip));
            return;
        }
    }
    if (g_route) SetRoute(false);
}

Vector3 Me() { return Call<Vector3>(GET_ENTITY_COORDS, Call<int>(PLAYER_PED_ID), 0); }

const char* RouteToClothingStore() {
    // GTA's own clothing store blips (sprite 73: Binco, Discount Store, Suburban, Ponsonbys).
    const Vector3 me = Me();
    float best = 1e12f, x = 0.0f, y = 0.0f;
    for (int b = Call<int>(GET_FIRST_BLIP_INFO_ID, 73); Call<BOOL>(DOES_BLIP_EXIST, b); b = Call<int>(GET_NEXT_BLIP_INFO_ID, 73)) {
        const Vector3 at = Call<Vector3>(GET_BLIP_INFO_ID_COORD, b);
        const float d = (at.x - me.x) * (at.x - me.x) + (at.y - me.y) * (at.y - me.y);
        if (d < best) best = d, x = at.x, y = at.y;
    }
    if (best >= 1e12f) return "No clothing store on the map";
    Call<void>(SET_NEW_WAYPOINT, x, y);
    return "GPS: nearest clothing store (Skate 3 boards)";
}

const char* RouteToSkateSpot() {
    const Vector3 me = Me();
    float best = 1e12f, x = 0.0f, y = 0.0f;
    for (const Spot& s : kSpots) {
        const float d = (s.x - me.x) * (s.x - me.x) + (s.y - me.y) * (s.y - me.y);
        if (d < best) best = d, x = s.x, y = s.y;
    }
    Call<void>(SET_NEW_WAYPOINT, x, y);
    return "GPS: nearest skate spot";
}

const char* ClearRoute() {
    Call<void>(SET_WAYPOINT_OFF);
    return "GPS: waypoint removed";
}

void Clear() {
    if (g_route) SetRoute(false);
    Spots(false);
}

} // namespace skatemap
