#include "ped_collision_native.h"
#include "ped_collision_native_detail.h"
#include "game_probe.h"
#include "ped_collider_builder.h"
#include "host_util.h"
#include "natives.h"
#include "pose_math.h"
#include "streaming.h"
#include <set>
#include <windows.h>
#include <filesystem>
#include <fstream>
#include <cstdio>
#include <cstring>
#include <array>
#include <algorithm>
#include <map>

namespace pedcollision {
namespace {
struct Part {
    detail::Entry entry;
    std::string path;
    std::uint32_t hash=0, raw=~0u;
    int entity=0, bone=-1;
};
std::vector<Part> g_parts;
// Runtime collider generation (any ped): templates, the output directory and
// the content-named models registered with the streamer this process.
builder::Templates g_templates;
std::filesystem::path g_cache;
struct Registered { std::uint32_t ydr=~0u, ytyp=~0u; };
std::map<std::string,Registered> g_registered;
// Names registered per process at most (21 limbs a character; a few
// characters and outfits). Each holds GTA streaming slots until exit.
constexpr std::size_t kRegistrationBudget=128;
// Collider models already built and on disk this process (named by limb
// identity; the first build stands for the process), so each is built and
// registered once rather than on every board take-out.
std::set<std::string> g_written;
bool g_started=false, g_type=false, g_failed=false, g_live=false;
int g_ped=0;
ULONGLONG g_requestedAt=0;
void (*g_log)(const char*)=nullptr;
void Log(const char* message) { if(g_log) g_log(message); }
void Remove() {
    for(auto& p:g_parts) {
        if(p.entity && gta::Call<bool>(gta::DOES_ENTITY_EXIST,p.entity)) gta::Call<void>(gta::DELETE_OBJECT,&p.entity);
        p.entity=0;p.bone=-1;
    }
    g_live=false;g_ped=0;
}
void Fail(const char* why) { Log(why);Remove();g_failed=true; }
void Collision(int entity,bool enabled) {
    gta::Call<void>(gta::SET_ENTITY_COLLISION,entity,enabled,false);
    gta::Call<void>(gta::SET_ENTITY_COMPLETELY_DISABLE_COLLISION,entity,enabled,false);
}
void Disable() { for(const auto& p:g_parts) if(p.entity) Collision(p.entity,false); }
}

bool Start(const std::wstring& ini,const std::wstring& dataRoot,void (*log)(const char*)) {
    Stop();g_log=log;g_failed=false;
    if(!GetPrivateProfileIntW(L"SkateV",L"PedPoseCollision",0,ini.c_str())) return false;
    // Collider templates (tools/ped_collider_templates.py); the colliders
    // themselves are built from whatever ragdoll compound the ped carries.
    std::string why;
    if(!builder::Load(std::filesystem::path(dataRoot)/L"private"/L"ped-colliders"/L"runtime",g_templates,why)) {
        Log(("posed NPC collision: "+why).c_str());return false;
    }
    wchar_t local[MAX_PATH]{};
    if(!GetEnvironmentVariableW(L"LOCALAPPDATA",local,MAX_PATH)) return false;
    g_cache=std::filesystem::path(local)/L"SkateV"/L"ped-colliders";
    std::error_code error;std::filesystem::create_directories(g_cache,error);
    if(error) { Log("posed NPC collision: collider cache directory unavailable");return false; }
    if(!streaming::Guards()) { Log("posed NPC collision: original streaming guards differ");return false; }
    g_requestedAt=0;g_started=true;return true;
}

namespace {
bool WriteOnce(const std::filesystem::path& path,const std::vector<std::uint8_t>& bytes) {
    std::error_code error;
    // Content names hash only the authored child, so a file left by older
    // templates (e.g. without the shader group) keeps its name and size:
    // reuse a cached file only when its bytes are identical.
    if(std::filesystem::is_regular_file(path,error) && std::filesystem::file_size(path,error)==bytes.size()) {
        std::ifstream in(path,std::ios::binary);
        std::vector<std::uint8_t> old(bytes.size());
        if(in.read(reinterpret_cast<char*>(old.data()),static_cast<std::streamsize>(old.size())) && old==bytes) return true;
    }
    const auto partial=path.wstring()+L".partial";
    { std::ofstream out(partial,std::ios::binary);
      if(!out.write(reinterpret_cast<const char*>(bytes.data()),static_cast<std::streamsize>(bytes.size()))) return false; }
    std::filesystem::rename(partial,path,error);
    return !error;
}

// Build (or reuse) every collider of the ped's live ragdoll compound.
bool Prepare(int ped,const probe::SkeletonInfo& skeleton,std::uint32_t count,std::vector<Part>& parts,std::string& why) {
    std::uintptr_t type=0;const char* reason="";
    if(!probe::PedFragmentType(ped,type,reason)) { why=reason;return false; }
    std::vector<builder::Child> children;
    probe::LiveMemory memory;
    if(!builder::ReadCompound(memory,type,children,why)) return false;
    parts.clear();
    const auto model=gta::Call<std::uint32_t>(gta::GET_ENTITY_MODEL,ped);
    for(std::size_t i=0;i<children.size();++i) {
        const auto& c=children[i];
        Part p;p.entry.model=builder::ModelName(model,i,c);p.entry.boneTag=c.boneTag;
        if(p.entry.model.empty()) { why="collider resource could not be built";return false; }
        p.bone=probe::BoneIndexByTag(skeleton,c.boneTag);
        if(p.bone<0||p.bone>=static_cast<int>(count)) { why="character lacks a ragdoll collision bone";return false; }
        const auto ydr=g_cache/(p.entry.model+".ydr"), ytyp=g_cache/(p.entry.model+".ytyp");
        // GTA streams these files whenever the model reloads (after the editor, say): a file
        // deleted mid-game (a cleanup, an antivirus) would be a fatal read in its streamer.
        std::error_code missing;
        if(!g_written.count(p.entry.model) || !std::filesystem::exists(ydr,missing) || !std::filesystem::exists(ytyp,missing)) {
            std::vector<std::uint8_t> sys;
            const auto& header=c.kind==builder::kCapsule?g_templates.capsule.header:g_templates.box.header;
            if(!builder::BuildYdr(g_templates,c,p.entry.model,sys)||!WriteOnce(ydr,builder::Rsc7File(header,sys))||
               !builder::BuildYtyp(g_templates,c,p.entry.model,sys)||!WriteOnce(ytyp,builder::Rsc7File(g_templates.ytypHeader,sys))) {
                why="collider resource could not be written";return false;
            }
            g_written.insert(p.entry.model);
        }
        p.path=streaming::NarrowAnsi(ydr);p.hash=util::Joaat(p.entry.model);
        if(p.path.empty()) { why="collider path not representable";return false; }
        parts.push_back(p);
    }
    return true;
}
}

void Tick(int ped,const float* boneWorld,std::uint32_t count,int ignoreVehicle) {
    if(!g_started || g_failed) return;
    if(!ped || !boneWorld || !count || count>512) { Disable();return; }
    if(g_ped!=ped) {
        Remove();
        probe::SkeletonInfo skeleton;const char* why="";
        if(!probe::ResolvePedSkeleton(ped,skeleton,why) || skeleton.count!=static_cast<int>(count)) return;
        std::string reason;
        if(!Prepare(ped,skeleton,count,g_parts,reason)) {
            char line[256];std::snprintf(line,sizeof(line),"posed NPC collision: disabled: %s",reason.c_str());
            Fail(line);return;
        }
        char line[200];std::snprintf(line,sizeof(line),"posed NPC collision: %zu colliders from the ped's own ragdoll compound",g_parts.size());
        Log(line);
        g_ped=ped;g_type=false;
    }
    static std::vector<posemath::Q> quaternions;
    quaternions.resize(g_parts.size());
    for(std::size_t i=0;i<g_parts.size();++i) {
        const auto& p=g_parts[i];
        if(p.bone<0 || p.bone>=static_cast<int>(count)) { Disable();return; }
        posemath::M m;std::memcpy(&m,boneWorld+16*p.bone,sizeof(m));
        if(!posemath::RigidQuaternion(m,quaternions[i])) { Disable();return; }
    }
    if(!g_type) {
        if(!streaming::Ready()) return;
        const auto reg=streaming::Registrar();
        for(auto& p:g_parts) {
            auto& r=g_registered[p.entry.model];
            if(r.ydr==~0u) {
                // ponytail: fixed budget, not GTA's real free count (its pool sits
                // behind obfuscated code); raise it if a cast of characters needs more.
                if(g_registered.size()>kRegistrationBudget) { Fail("posed NPC collision: registration budget spent this session");return; }
                // A content-named model nobody else may already own.
                if(gta::Call<bool>(gta::IS_MODEL_VALID,p.hash)||gta::Call<bool>(gta::IS_MODEL_IN_CDIMAGE,p.hash)) { Fail("posed NPC collision: model name already occupied");return; }
                const auto ydrName=p.entry.model+".ydr", ytypName=p.entry.model+".ytyp";
                const auto ytypPath=streaming::NarrowAnsi(g_cache/ytypName);
                reg(&r.ydr,p.path.c_str(),true,ydrName.c_str(),false,false);
                if(r.ydr==~0u) { Fail("posed NPC collision: drawable registration failed");return; }
                reg(&r.ytyp,ytypPath.c_str(),true,ytypName.c_str(),false,false);
                if(r.ytyp==~0u) { Fail("posed NPC collision: type registration failed");return; }
                streaming::TypeLoader()(ytypName.c_str());
            }
            p.raw=r.ydr;
        }
        g_type=true;g_requestedAt=GetTickCount64();
        Log("posed NPC collision: generated limb resources requested");
    }
    bool loaded=true;
    for(const auto& p:g_parts) {
        if(!gta::Call<bool>(gta::IS_MODEL_VALID,p.hash)||!gta::Call<bool>(gta::HAS_MODEL_LOADED,p.hash)) {
            if(gta::Call<bool>(gta::IS_MODEL_VALID,p.hash)) gta::Call<void>(gta::REQUEST_MODEL,p.hash);
            loaded=false;
        }
    }
    if(!loaded) {
        if(!g_requestedAt) g_requestedAt=GetTickCount64();
        if(GetTickCount64()-g_requestedAt>15000) Fail("posed NPC collision: model loading timed out");
        return;
    }
    for(std::size_t i=0;i<g_parts.size();++i) {
        auto& p=g_parts[i];const auto* m=boneWorld+16*p.bone;const auto& q=quaternions[i];
        if(p.entity && !gta::Call<bool>(gta::DOES_ENTITY_EXIST,p.entity)) { Fail("posed NPC collision: limb instance removed unexpectedly");return; }
        if(!p.entity) {
            p.entity=gta::Call<int>(gta::CREATE_OBJECT_NO_OFFSET,p.hash,m[12],m[13],m[14],false,false,false,0);
            if(!p.entity) { Fail("posed NPC collision: native object creation failed");return; }
            Collision(p.entity,false);
            gta::Call<void>(gta::SET_ENTITY_AS_MISSION_ENTITY,p.entity,true,false);
            gta::Call<void>(gta::SET_ENTITY_VISIBLE,p.entity,false,false);
            gta::Call<void>(gta::FREEZE_ENTITY_POSITION,p.entity,true);
            gta::Call<void>(gta::SET_ENTITY_HAS_GRAVITY,p.entity,false);
        }
        gta::Call<void>(gta::SET_ENTITY_COORDS_NO_OFFSET,p.entity,m[12],m[13],m[14],false,false,false);
        gta::Call<void>(gta::SET_ENTITY_QUATERNION,p.entity,q.x,q.y,q.z,q.w);
    }
    // The skitched vehicle is GTA's to drive: the rider's limbs pass through it and the riders on it.
    if(ignoreVehicle && gta::Call<bool>(gta::DOES_ENTITY_EXIST,ignoreVehicle)) {
        const int seats[]={-1,0};
        for(const auto& p:g_parts) {
            gta::Call<void>(gta::SET_ENTITY_NO_COLLISION_ENTITY,p.entity,ignoreVehicle,true);
            for(int seat:seats) {
                const int rider=gta::Call<int>(gta::GET_PED_IN_VEHICLE_SEAT,ignoreVehicle,seat,0);
                if(rider) gta::Call<void>(gta::SET_ENTITY_NO_COLLISION_ENTITY,p.entity,rider,true);
            }
        }
    }
    // All shapes have valid poses before any shape can contact an NPC.
    for(const auto& p:g_parts) Collision(p.entity,true);
    if(!g_live) {
        char line[160];std::snprintf(line,sizeof(line),"posed NPC collision: %zu native kinematic limb instances active; GTA contact verification pending",g_parts.size());
        Log(line);g_live=true;
    }
}
void Stop() { Remove();g_started=false; }
}
