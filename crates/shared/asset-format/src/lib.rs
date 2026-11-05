//! Asset blob and manifest formats: the contract between `asset-pipeline` and the client.
//!
//! Every blob is `magic[4] | version u16 LE | postcard payload`; encoding is deterministic.

mod anim;
mod anim2;
mod art_table;
mod audio_table;
mod blob;
mod client_data;
mod codec;
mod error;
mod fsm;
mod fx;
mod groups;
mod hash;
mod id;
mod manifest;
mod model;
mod pieces;
mod quant;
mod sound3d_table;
mod soundscape_table;
mod texture;
mod ui_index;
mod world_settings;

pub use anim::{ANIM_MAGIC, AnimationBlob, Track};
pub use anim2::{ANIM_PACK_MAGIC, AnimPack, Channel, Clip, ROT_STRIDE, TrackKeys, VEC_STRIDE};
pub use art_table::{
    ART_TABLE_MAGIC, ArtRecord, ArtTable, BuildingArt, BuildingTypeArt, BuildingVariant, CityArt,
    CityArtB2, CivArt, FeatureArt, Figure, FigureB2, Formation, LandmarkArt, NATURAL_WONDER_ART,
    OverlayArt, PlayerColor, Projection, RiverArt, RouteArt, RouteModel, SvArt, TerrainArt,
    TreeArt, UnitArt, UnitArtB2, UnitRecord, UnitRecordB2, UnitVariant, UnitVariantB2,
    VisEffectArt, WallArt, WonderArt, WonderState, is_natural_wonder_art,
};
pub use audio_table::{
    AUDIO_TABLE_ID, AUDIO_TABLE_MAGIC, AudioTable, LinkTable, SoundDef, SoundKind, SoundLink,
    SoundLoad, SoundScript,
};
pub use blob::FORMAT_VERSION;
pub use client_data::{CURSORS_ID, CURSORS_MAGIC, CursorArt, CursorSet, DGE_GRAPHS_ID};
pub use codec::{Codec, MAX_SECTION_LEN, Section, shuffle_u16, unshuffle_u16};
pub use error::FormatError;
pub use fsm::{FSM_MAGIC, FsmCondition, FsmState, IdleFsm, Transition};
pub use fx::{
    FX_INDEX_ID, FX_INDEX_MAGIC, FX_MAGIC, FxEffect, FxEmitter, FxForce, FxFrame, FxIndex, FxPath,
    FxProps, FxShader, FxTrack, FxVisEffect, flag as fx_flag, flag2 as fx_flag2,
};
pub use groups::{
    BOOT_GROUP, GROUP_TABLE_ID, GROUP_TABLE_MAGIC, Group, GroupTable, PriorityClass, Trigger,
};
pub use hash::{ContentHash, ParseHashError};
pub use id::LogicalId;
pub use manifest::{
    MANIFEST_MAGIC, Manifest, PlatformProfile, PrefetchGroup, Variant, VariantFormat,
};
pub use model::{
    AnimEvent, AnimRef, AttrFormat, Bone, ClipKey, ClipRef, ConstParam, EffectRef, Indices,
    MODEL_MAGIC, Material, MaterialDesc, Mesh, MeshArrays, MeshV2, ModelBlob, ModelBlobV2,
    PackedIndices, ParamSet, Part, STATE_INTACT, STATES_ALL, Semantic, Skeleton, StateClip,
    TexKind, TexParam, TexSource, Transform, UV_F16_LIMIT, VertexAttr,
};
pub use pieces::{HeightDiff, PIECES_MAGIC, PieceData, PieceStyle, TerrainPiece, TerrainPieceSet};
pub use quant::{
    f16_bits_to_f32, f32_to_f16_bits, oct_decode, oct_encode, quantize_weights, quat_pack,
    quat_unpack,
};
pub use sound3d_table::{
    DEFAULT_CUTOFF_DISTANCE, DEFAULT_MIN_DISTANCE, ModelSounds, PitchRange, SOUND3D_TABLE_ID,
    SOUND3D_TABLE_MAGIC, Script3D, Sound3DTable, SoundTrigger,
};
pub use soundscape_table::{
    SOUNDSCAPE_TABLE_ID, SOUNDSCAPE_TABLE_MAGIC, ScapeBinding, ScapeBindingKind, ScapeElement,
    Soundscape, SoundscapeTable,
};
pub use texture::{
    TEXTURE_MAGIC, TexDim, TextureBlob, TextureFormat, pad_to_block, stored_content, stored_shrink,
};
pub use ui_index::{
    LOCALE_MAGIC, LocaleGrammar, LocaleTable, UI_INDEX_ID, UI_INDEX_MAGIC, UI_TABLES_ID,
    UI_TABLES_MAGIC, UiAtlas, UiFileInfo, UiFileKind, UiFont, UiGroup, UiIconMapping, UiIndex,
    UiLanguage, UiLayerInfo, UiTables, UiTexDim, bare_name, ui_file_id, ui_sound_file_id,
    ui_text_id, ui_texture_id,
};
pub use world_settings::{
    Arrows, COAST_NONE, CoastLevel, CoastProfile, CoastSettings, CoastStyle, ColorKey,
    CultureBorders, DecalDef, Extra, FluidFow, HexGrid, HighlightKind, HighlightStyle, IceParams,
    IceSettings, LeanInfo, OverlayDefs, Rotation, TerrainDefRow, UnitSelectionHighlight,
    WORLD_SETTINGS_ID, WORLD_SETTINGS_MAGIC, WaveSettings, WorldSettings,
};
