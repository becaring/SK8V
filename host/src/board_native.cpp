#include "board_native.h"
#include "board_native_detail.h"
#include "game_probe.h"
#include "host_util.h"
#include "natives.h"
#include "streaming.h"
#include <windows.h>
#include <cstring>
#include <filesystem>
#include <cstdio>

namespace boardnative {
namespace {
using posemath::M;
constexpr std::uint32_t Model=util::Joaat("skatev_board");
void (*g_log)(const char*)=nullptr;
std::string g_ydr,g_ytyp; int g_object=0;
bool g_started=false,g_registered=false,g_type=false,g_failed=false;
bool g_poseReported=false;
std::uint32_t g_ydrId=~0u,g_ytypId=~0u;
ULONGLONG g_begin=0;
// Bone indices of the seven board tags, for the skeleton they were read from.
std::uintptr_t g_indexedSkeleton=0; int g_indices[7];
void Log(const char* s){if(g_log)g_log(s);}
bool Pose(const SvBoardPose& pose) {
    probe::SkeletonInfo s; const char* why=nullptr;
    if(!probe::ResolvePedSkeleton(g_object,s,why)||s.count!=7){if(!g_poseReported){char line[256];std::snprintf(line,sizeof(line),"native board: skeleton unavailable (%s, bones=%d); object hidden",why?why:"expected seven bones",s.count);Log(line);g_poseReported=true;}return false;}
    if(s.skeleton!=g_indexedSkeleton) {
        g_indexedSkeleton=0;
        for(int i=0;i<7;++i) { g_indices[i]=probe::BoneIndexByTag(s,static_cast<std::uint16_t>(i)); if(g_indices[i]<0||g_indices[i]>=7)return false; }
        g_indexedSkeleton=s.skeleton;
    }
    const int* indices=g_indices;
    __try {
        const auto data=*reinterpret_cast<const std::uintptr_t*>(s.skeleton);
        const auto parents=*reinterpret_cast<const std::int16_t* const*>(data+0x38);
        if(!parents||!s.parentMtx||!s.objectMtx||!s.globalMtx)return false;
        for(int i=0;i<7;++i) if(parents[indices[i]]!=(detail::Parents[i]<0?-1:indices[detail::Parents[i]]))return false;
        M world[7],objects[7],locals[7],entity;
        std::memcpy(world,pose.world,sizeof(world)); std::memcpy(&entity,reinterpret_cast<void*>(s.parentMtx),64);
        if(!detail::Convert(world,entity,objects,locals))return false;
        for(int i=0;i<7;++i) for(int r=0;r<4;++r) for(int c=0;c<3;++c) {
            reinterpret_cast<float*>(s.objectMtx)[indices[i]*16+r*4+c]=locals[i].r[r][c];
            reinterpret_cast<float*>(s.globalMtx)[indices[i]*16+r*4+c]=objects[i].r[r][c];
        }
        return true;
    } __except(EXCEPTION_EXECUTE_HANDLER){return false;}
}
void Remove(){if(g_object&&gta::Call<bool>(gta::DOES_ENTITY_EXIST,g_object))gta::Call<void>(gta::DELETE_OBJECT,&g_object);g_object=0;}
bool Hide(){if(g_object&&gta::Call<bool>(gta::DOES_ENTITY_EXIST,g_object))gta::Call<void>(gta::SET_ENTITY_VISIBLE,g_object,false,false);return false;}
void Fail(const char* why){Log(why);Remove();g_failed=true;}
}
bool Start(const std::wstring& iniPath,const std::wstring& dataRoot,void (*log)(const char*)) {
    Stop(); g_log=log;g_failed=false;g_poseReported=false;
    if(!GetPrivateProfileIntW(L"SkateV",L"BoardNative",1,iniPath.c_str()))return false;
    const auto root=std::filesystem::path(dataRoot)/L"private"/L"board-native";
    const auto ydr=root/L"skatev_board.ydr",ytyp=root/L"skatev_board.ytyp";
    if(!std::filesystem::is_regular_file(ydr)||!std::filesystem::is_regular_file(ytyp)){Log("native board: private YDR/YTYP missing");return false;}
    auto dr=streaming::NarrowAnsi(ydr),ty=streaming::NarrowAnsi(ytyp);
    if(dr.empty()||ty.empty()||((g_ydrId!=~0u||g_ytypId!=~0u)&&(dr!=g_ydr||ty!=g_ytyp))){Log("native board: unsupported or changed resource path");return false;}
    if(!streaming::Guards()){Log("native board: 3889 streaming function guards differ");return false;}
    g_ydr=dr;g_ytyp=ty;g_started=true;g_begin=GetTickCount64();return true;
}
bool Tick(bool skating,const SvBoardPose& pose) {
    if(!g_started||g_failed)return false;
    if(!skating){Remove();return false;}
    if(pose.size!=sizeof(SvBoardPose)||pose.bone_count!=7)return Hide();
    for(const auto& bone:pose.world)for(float f:bone)if(!std::isfinite(f))return Hide();
    M entity;std::memcpy(&entity,pose.entity,sizeof(entity));posemath::Q quaternion;if(!posemath::RigidQuaternion(entity,quaternion))return Hide();
    if(!g_type) {
        g_begin=GetTickCount64();
        if(!streaming::Ready())return false;
        if(!g_registered) {
            if(gta::Call<bool>(gta::IS_MODEL_IN_CDIMAGE,Model)||gta::Call<bool>(gta::IS_MODEL_VALID,Model)){Fail("native board: custom model name already occupied");return false;}
            const auto reg=streaming::Registrar();
            if(g_ydrId==~0u)reg(&g_ydrId,g_ydr.c_str(),true,"skatev_board.ydr",false,false);
            if(g_ytypId==~0u)reg(&g_ytypId,g_ytyp.c_str(),true,"skatev_board.ytyp",false,false);
            if(g_ydrId==~0u||g_ytypId==~0u){Fail("native board: raw resource registration failed");return false;}
            g_registered=true;
        }
        streaming::TypeLoader()("skatev_board.ytyp");g_type=true;Log("native board: custom YDR/YTYP registered; archetype requested");
    }
    if(!gta::Call<bool>(gta::IS_MODEL_VALID,Model)||!gta::Call<bool>(gta::IS_MODEL_IN_CDIMAGE,Model)||!gta::Call<bool>(gta::HAS_MODEL_LOADED,Model)) {
        if(gta::Call<bool>(gta::IS_MODEL_VALID,Model))gta::Call<void>(gta::REQUEST_MODEL,Model);
        if(GetTickCount64()-g_begin>15000)Fail("native board: model load timed out");return false;
    }
    if(!g_object) {
        // The pinned Legacy DB and owned +CDCB74 wrapper both require eight
        // slots; +37F4D7D reads p7 at args+0x38 even in a local script.
        g_object=gta::Call<int>(gta::CREATE_OBJECT_NO_OFFSET,Model,pose.entity[12],pose.entity[13],pose.entity[14],false,false,false,0);
        if(!g_object){Fail("native board: CREATE_OBJECT_NO_OFFSET failed");return false;}
        gta::Call<void>(gta::SET_ENTITY_VISIBLE,g_object,false,false);
        gta::Call<void>(gta::SET_ENTITY_AS_MISSION_ENTITY,g_object,true,false);gta::Call<void>(gta::FREEZE_ENTITY_POSITION,g_object,true);
        gta::Call<void>(gta::SET_ENTITY_HAS_GRAVITY,g_object,false);gta::Call<void>(gta::SET_ENTITY_DYNAMIC,g_object,false);
        gta::Call<void>(gta::SET_ENTITY_COLLISION,g_object,false,false);
        // Despite the modern name, this Legacy native uses collision-enable
        // polarity, matching the measured ped collisionDisabled=1 gate.
        gta::Call<void>(gta::SET_ENTITY_COMPLETELY_DISABLE_COLLISION,g_object,false,false);
        Log("native board: presentation object created; physics and collision disabled");
    }
    if(!gta::Call<bool>(gta::DOES_ENTITY_EXIST,g_object)){g_object=0;return false;}
    gta::Call<void>(gta::SET_ENTITY_COORDS_NO_OFFSET,g_object,pose.entity[12],pose.entity[13],pose.entity[14],false,false,false);
    gta::Call<void>(gta::SET_ENTITY_QUATERNION,g_object,quaternion.x,quaternion.y,quaternion.z,quaternion.w);
    const bool posed=Pose(pose);
    if(!posed&&!g_poseReported){Log("native board: invalid bone tags, parents, or matrices; object hidden");g_poseReported=true;}
    gta::Call<void>(gta::SET_ENTITY_VISIBLE,g_object,posed,false);return posed;
}
void Stop(){Remove();g_started=false;}
int Entity(){return g_object;}
}
