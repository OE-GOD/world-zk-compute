/// Poseidon sponge hash for BN254 Fq field.
///
/// Matches PoseidonSponge.sol exactly.
/// Parameters: width=3, rate=2, 8 full rounds (4+4), 57 partial rounds, x^5 S-box.
/// Uses PSE/Scroll BN254 constants (Grain LFSR generated).
///
/// Initial state: [2^64, 0, 0] (capacity domain separator).
use crate::field::{Fq, U256};

/// Total number of rounds: 4 full + 57 partial + 4 full = 65.
const NUM_ROUNDS: usize = 65;
/// First 4 rounds are full rounds.
const FULL_FIRST: usize = 4;
/// Last 4 rounds are full rounds (rounds 61..64).
const FULL_LAST_START: usize = 61;

/// Round constants for all 65 rounds, 3 constants per round.
/// Stored as raw `[u64; 4]` limbs to avoid `Fq(U256(...))` wrapper overhead
/// in the WASM data section. Converted to `Fq` at use time via `rc()`.
#[rustfmt::skip]
const ROUND_CONSTANTS: [[[u64; 4]; 3]; NUM_ROUNDS] = [
    // Round 0
    [[0x8d21d47304cd8e6e, 0x14c4993c11bb2993, 0xd05986d656f40c21, 0x0ee9a592ba9a9518], [0x5696fff40956e864, 0x887b08d4d00868df, 0x5986587169fc1bcd, 0x00f1445235f2148c], [0xe879f3890ecf73f5, 0x30c728730b7ab36c, 0x1f29a058d0fa80b9, 0x08dff3487e8ac99e]],
    // Round 1
    [[0x20966310fadc01d0, 0x56c35342c84bda6e, 0xc3ce28f7532b13c8, 0x2f27be690fdaee46], [0x8b8327bebca16cf2, 0xb763fe04b8043ee4, 0x2416bebf3d4f6234, 0x2b2ae1acf68b7b8d], [0xe64b44c7dbf11cfa, 0x5952c175ab6b03ea, 0xcca5eac06f97d4d5, 0x0319d062072bef7e]],
    // Round 2
    [[0x8ef7b387bf28526d, 0xc8b7bf27ad49c629, 0x8a376df87af4a63b, 0x28813dcaebaeaa82], [0x150928adddf9cb78, 0x2033865200c352bc, 0xf181bf38e1c1d40d, 0x2727673b2ccbc903], [0xb8fb9e31e65cc632, 0x6efbd43e340587d6, 0xe74abd2b2a1494cd, 0x234ec45ca27727c2]],
    // Round 3
    [[0xcd99ff6e8797d428, 0xab10a8150a337b1c, 0x7f862cb2cf7cf760, 0x15b52534031ae18f], [0xd701d4eecf68d1f6, 0x8e0e8a8d1b58b132, 0x5ed9a3d186b79ce3, 0x0dc8fad6d9e4b35f], [0x97805518a47e4d9c, 0xea4eb378f62e1fec, 0x600f705fad3fb567, 0x1bcd95ffc211fbca]],
    // Round 4
    [[0x17cb978d069de559, 0xc76da36c25789378, 0xe9eff81b016fc34d, 0x10520b0ab721cadf], [0xe88a9eb81f5627f6, 0x2932498075fed0ac, 0x9b257d8ed5fbbaf4, 0x1f6d48149b8e7f7d], [0xca34bdb5460c8705, 0xfff8dc1c816f0dc9, 0xd29e00ef35a2089b, 0x1d9655f652309014]],
    // Round 5
    [[0x8fe3d4185697cc7d, 0xa731ff67e4703205, 0xb051f7b1cd43a99b, 0x04df5a56ff95bcaf], [0xf6ec282b6e4be828, 0x8690a10a8c8424a7, 0x151b3d290cedaf14, 0x0672d995f8fff640], [0x9fc1d8209b5c75b9, 0x0c9a9dcc06f2708e, 0xb21200d7ffafdd5f, 0x099952b414884454]],
    // Round 6
    [[0x83fd0e843a6b9fa6, 0x48e43586a9b4cd91, 0x7c483143ba8d4694, 0x052cba2255dfd00c], [0x16077cb93c464ddc, 0x82de55707251ad77, 0xb0bd74712b7999af, 0x0b8badee690adb8e], [0xb963d0a8e4b2bdd1, 0x49c15d60683a8050, 0x5a1ee651020c07c7, 0x119b1590f13307af]],
    // Round 7
    [[0xce15be0bfb4a8d09, 0x2c4acfc884ef4ee5, 0x2529d36be0f67b83, 0x03150b7cd6d5d17b], [0xbe69cb317c9ea565, 0x5374efb83d80898a, 0x3cf1951f17391235, 0x2cc6182c5e14546e], [0x92d2cd73111bf0f9, 0x4218cadedac14e2b, 0x50cfe129a404b376, 0x005032551e6378c4]],
    // Round 8
    [[0x88f9da2cc28276b5, 0x6469c399fcc069fb, 0xbb147e972ebcb951, 0x233237e3289baa34], [0xe80c2d4c24d60280, 0x23037f21b34ae5a4, 0xc980d31674bfbe63, 0x05c8f4f4ebd4a6e3], [0xee1f09b2590fc65b, 0x52bcf35ef3aeed91, 0xba05d818a319f252, 0x0a7b1db13042d396]],
    // Round 9
    [[0x5df542365a404ec0, 0xf156e2b086ff47dc, 0xb14296572c9d32db, 0x2a73b71f9b210cf5], [0x76a760bb5c50c460, 0xec18f2c4dbe7f229, 0x935107e9ffc91dc3, 0x1ac9b0417abcc9a1], [0x9015ee046dc93fc0, 0x269f3e4d6cb10434, 0x3fabb076707ef479, 0x12c0339ae0837482]],
    // Round 10
    [[0x8246682e56e9a28e, 0x52900aa3253baac6, 0x7f5b18db4e1e704f, 0x0b7475b102a165ad], [0x32ab3aa88d7f8448, 0x7c843e379366f2ea, 0xdb1c5e49f6e8b891, 0x037c2849e191ca3e], [0x45fdb176a716346f, 0xd5206c5c93a07dc1, 0xe92674661e217e9b, 0x05a6811f8556f014]],
    // Round 11
    [[0x7b675ef5f38bd66e, 0x4076e87a7b2883b4, 0x6e947b75d54e9f04, 0x29a795e7d9802894], [0x507be199981fd22f, 0x6e8c7382c8a1585c, 0x45a3857afc18f582, 0x20439a0c84b322eb], [0x4a2a6f2a0982c887, 0xbb50f27799a84b6d, 0x94ec2050c7371ff1, 0x2e0ba8d94d9ecf4a]],
    // Round 12
    [[0xe6d0ddcca17d71c8, 0x17822cd2109048d2, 0xca38eb7cce822b45, 0x143fd115ce08fb27], [0xc84323623be9caf1, 0xf8611659323dbcbf, 0x57968dbbdcf813cd, 0x0c64cbecb1c734b8], [0xf1426cef9403da53, 0xe74f348d62c2b670, 0x46fca925c163ff5a, 0x028a305847c683f6]],
    // Round 13
    [[0x24d6755b5db9e30c, 0x6a6bcb64d89427b8, 0x5fa940ab4c4380f2, 0x2e4ef510ff0b6fda], [0xb96384f50579400e, 0x8925b4f6d033b078, 0x63d79270c956ce3b, 0x0081c95bc43384e6], [0xba8a9f4023a0bb38, 0xe2491b349c039a0b, 0x187e2fade687e05e, 0x2ed5f0c91cbd9749]],
    // Round 14
    [[0x990f01f33a735206, 0x3448a22c76234c8c, 0x4bbf374ed5aae2f0, 0x30509991f88da350], [0xa7529094424ec6ad, 0xf0a1119fb2067b41, 0x221b7c4d49a356b9, 0x1c3f20fd55409a53], [0x170887b47ddcb96c, 0xc46bb2213e8e131e, 0x049514459b6e18ee, 0x10b4e7f3ab5df003]],
    // Round 15
    [[0x039aa3502e43adef, 0xdd80f804c077d775, 0x3ddd543d891c2abd, 0x2a1982979c3ff7f4], [0x5cad0f1315bd5c91, 0xba431ebc396c9af9, 0xfeddbead56d6d55d, 0x1c74ee64f15e1db6], [0x9c2fe45a0ae146a0, 0x9e4f2e8b82708cfa, 0xeab9303cace01b4b, 0x07533ec850ba7f98]],
    // Round 16
    [[0x8a11abf3764c0750, 0x285c68f42d42c180, 0xa151e4eeaf17b154, 0x21576b438e500449], [0x743d6930836d4a9e, 0xbce8384c815f0906, 0x08ad5ca193d62f10, 0x2f17c0559b8fe796], [0xe665b0b1b7e2730e, 0x9775a4201318474a, 0xa79e8aae946170bc, 0x2d477e3862d07708]],
    // Round 17
    [[0xd89be0f5b2747eab, 0xafba2266c38f5abc, 0x90e095577984f291, 0x162f5243967064c3], [0x7777a70092393311, 0xd7a8596a87f29f8a, 0x264ecd2c8ae50d1a, 0x2b4cb233ede9ba48], [0x4254e7c35e03b07a, 0x6db2eece6d85c4cf, 0x1dbaf8f462285477, 0x2c8fbcb2dd8573dc]],
    // Round 18
    [[0xe5e88db870949da9, 0x9e1b61e9f601e9ad, 0xf2ff453f0cd56b19, 0x1d6f347725e4816a], [0x4cd49af5c4565529, 0xf9e6ac02b68d3132, 0xebc2d8b3df5b913d, 0x204b0c397f4ebe71], [0x4ff8fb75bc79c502, 0x9ecb827cd7dc2553, 0x4f1149b3c63c3c2f, 0x0c4cb9dc3c4fd817]],
    // Round 19
    [[0x9a616ddc45bc7b54, 0x1e5c49475279e063, 0xa25416474f493030, 0x174ad61a1448c899], [0x3a9816d49a38d2ef, 0xeaaa28c177cc0fa1, 0xf759df4ec2f3cde2, 0x1a96177bcf4d8d89], [0x8242ace360b8a30a, 0x05202c126a233c1a, 0xd0ef8054bc60c4ff, 0x066d04b24331d71c]],
    // Round 20
    [[0x27037a62aa1bd804, 0x381cc65f72e02ad5, 0x2195782871c6dd3b, 0x2a4c4fc6ec0b0cf5], [0xe55afc01219fd649, 0x5e727f8446f6d9d7, 0x47e9f2e14a7cedc9, 0x13ab2d136ccf37d4], [0x4c2e3e869acc6a9a, 0xc1b04fcec26f5519, 0x19d24d843dc82769, 0x1121552fca260616]],
    // Round 21
    [[0x09a5546c7c97cff1, 0xa6cd267d595c4a89, 0x889bc81715c37d77, 0x00ef653322b13d6c], [0x845aca35d8a397d3, 0x400c776d652595d9, 0x8b261d8ba74051e6, 0x0e25483e45a66520], [0x46448db979eeba89, 0x395ac3d4dde92d8c, 0x245264659e15d88e, 0x29f536dcb9dd7682]],
    // Round 22
    [[0x0e456baace0fa5be, 0x5a124e2780bbea17, 0xdfda33575dbdbd88, 0x2a56ef9f2c53feba], [0xee416240a8cb9af1, 0xf2ae2999a46762e8, 0xecfb7a2d17b5c409, 0x1c8361c78eb5cf5d], [0xd3d0ab4be74319c5, 0x83e8e68a764507bf, 0xc0473089aaf0206b, 0x151aff5f38b20a0f]],
    // Round 23
    [[0xe76e47615b51f100, 0xa9f52fc8c8b6cdd1, 0xc1b239c88f7f9d43, 0x04c6187e41ed881d], [0x9e801b7ddc9c2967, 0x4b81c61ed1577644, 0x10d84331f6fb6d53, 0x13b37bd80f4d27fb], [0x9321ceb1c4e8a8e4, 0x2ce3664c2a52032c, 0xf578bfbd32c17b7a, 0x01a5c536273c2d9d]],
    // Round 24
    [[0x832239065b7c3b02, 0x4a9a2c666b9726da, 0x5ad05f5d7acb950b, 0x2ab3561834ca7383], [0x9f7ed516a597b646, 0xacaf6af4e95d3bf6, 0x200fe6d686c0d613, 0x1d4d8ec291e720db], [0x1514c9c80b65af1d, 0xb925351240a04b71, 0x8f5784fe7919fd2b, 0x041294d2cc484d22]],
    // Round 25
    [[0x042971dd90e81fc6, 0x98f57939d126e392, 0x1c4fa715991f0048, 0x154ac98e01708c61], [0x4524563bc6ea4da4, 0x50b3684c88f8b0b0, 0x3eedd84093aef510, 0x0b339d8acca7d4f8], [0x81ed95b50839c82e, 0x98f0e71eaff4a7dd, 0x54a4f84cfbab3445, 0x0955e49e6610c942]],
    // Round 26
    [[0x3525401ea0654626, 0xa9a6f41e6f535c6f, 0x26b9e22206f15abc, 0x06746a6156eba544], [0xac917c7ff32077fb, 0x38e5790e2bd0a196, 0x496f3820c549c278, 0x0f18f5a0ecd1423c], [0x2a738223d6f76e13, 0x4bb563583ede7bc9, 0x8ac59eff5beb261e, 0x04f6eeca1751f730]],
    // Round 27
    [[0xc1768d26fc0b3758, 0x8811eb116fb3e45b, 0xc1a3ec4da3cdce03, 0x2b56973364c4c4f5], [0x83feb65d437f29ef, 0x8e1392b385716a5d, 0xdcd76b89804b1bcb, 0x123769dd49d5b054], [0x94257b2fb01c63e9, 0xa989f64464711509, 0x88ee52b91169aace, 0x2147b424fc48c80a]],
    // Round 28
    [[0xea54ad897cebe54d, 0x647e6f34ad4243c2, 0x1a6c5505ea332a29, 0x0fdc1f58548b8570], [0x944f685cc0a0b1f2, 0xbceff28c5dbbe0c3, 0xdf68abcf0f7786d4, 0x12373a8251fea004], [0xdd8a1f35c1a90035, 0xa642756b6af44203, 0xad7ea52ff742c9e8, 0x21e4f4ea5f35f85b]],
    // Round 29
    [[0x8a81934f1bc3b147, 0xb57366492f45e90d, 0xdfb4722224d4c462, 0x16243916d69d2ca3], [0xa13a4159cac04ac2, 0xabc21566e1a0453c, 0xf66f9adbc88b4378, 0x1efbe46dd7a578b4], [0x3b672cc96a88969a, 0xd468d5525be66f85, 0x8886020e23a7f387, 0x07ea5e8537cf5dd0]],
    // Round 30
    [[0xa9fe16c0b76c00bc, 0x650f19a75e7ce11c, 0xb7b478a30f9a5b63, 0x05a8c4f9968b8aa3], [0x2d9d57b72a32e83f, 0x3f7818c701b9c788, 0xfbfe59bd345e8dac, 0x20f057712cc21654], [0x9bd90b33eb33db69, 0x6dcd8e88d01d4901, 0x9672f8c67fee3163, 0x04a12ededa9dfd68]],
    // Round 31
    [[0xe49ec9544ccd101a, 0xbd136ce5091a6767, 0xe44f1e5425a51dec, 0x27e88d8c15f37dce], [0x176c41ee433de4d1, 0x6e096619a7703223, 0xb8a5c8c5e95a41f6, 0x2feed17b84285ed9], [0x6972b8bd53aff2b8, 0x94e5942911312a0d, 0x404241420f729cf3, 0x1ed7cc76edf45c7c]],
    // Round 32
    [[0xdf2874be45466b1a, 0xac6783476144cdca, 0x157ff8c586f5660e, 0x15742e99b9bfa323], [0x284f033f27d0c785, 0x77107454c6ec0317, 0xc895fc6887ddf405, 0x1aac285387f65e82], [0xec75a96554d67c77, 0x832e2e7a49775f71, 0xf9ddadbdb6057357, 0x25851c3c845d4790]],
    // Round 33
    [[0x0ddccc3d9f146a67, 0x53b7ebba2c552337, 0xce78457db197edf3, 0x15a5821565cc2ec2], [0x2f15485f28c71727, 0xdcf64f3604427750, 0x0efa7e31a1db5966, 0x2411d57a4813b998], [0x58828b5ef6cb4c9b, 0x47e9a98e12f4cd25, 0x13e335b8c0b6d2e6, 0x002e6f8d6520cd47]],
    // Round 34
    [[0x398834609e0315d2, 0xaf8f0e91e2fe1ed7, 0x97da00b616b0fcd1, 0x2ff7bc8f4380cde9], [0xe93be4febb0d3cbe, 0x2e9521f6b7bb68f1, 0x5ee02724471bcd18, 0x00b9831b94852559], [0x7d77adbf0c9c3512, 0x1ca408648a4743a8, 0x86913b0e57c04e01, 0x0a2f53768b8ebf6a]],
    // Round 35
    [[0x7f2a290305e1198d, 0x0f599ff7e94be69b, 0x3a479f91ff239e96, 0x00248156142fd037], [0x50eb512a2b2bcda9, 0x397196aa6a542c23, 0x28cf8c02ab3f0c9a, 0x171d5620b87bfb13], [0x9d1045e4ec34a808, 0x60c952172dd54dd9, 0x70087c7c10d6fad7, 0x170a4f55536f7dc9]],
    // Round 36
    [[0x482eca17e2dbfae1, 0xcc37e38c1cd211ba, 0x2ef3134aea04336e, 0x29aba33f799fe66c], [0xb5ba650369e64973, 0xe70d114a03f6a0e8, 0xfdd1bb1945088d47, 0x1e9bc179a4fdd758], [0x9c9e1c43bdaf8f09, 0xfeaad869a9c4b44f, 0x58f7f4892dfb0b5a, 0x1dd269799b660fad]],
    // Round 37
    [[0x5d1dd2cb0f24af38, 0x7ccd426fe869c7c9, 0x401181d02e15459e, 0x22cdbc8b70117ad1], [0xd5ba93b9c7dacefd, 0xfd3150f52ed94a7c, 0x3a9f57a55c503fce, 0x0ef042e454771c53], [0x3b304ffca62e8284, 0x1318e8b08a0359a0, 0xf287f3036037e885, 0x11609e06ad6c8fe2]],
    // Round 38
    [[0x08b08f5b783aa9af, 0xfecd58c076dfe427, 0x9e753eea427c17b7, 0x1166d9e554616dba], [0xf855a888357ee466, 0x177fbf4cd2ac0b56, 0x93413026354413db, 0x2de52989431a8595], [0x74bf01cf5f71e9ad, 0xf51aee5b17b8e89d, 0x9a6da492f3a8ac1d, 0x3006eb4ffc7a8581]],
    // Round 39
    [[0x62344c8225145086, 0x2993fe8f0a4639f9, 0xfdcf6fff9e3f6f42, 0x2af41fbb61ba8a80], [0x81b214bace4827c3, 0x8718ab27889e85e7, 0xe5a6b41a8ebc85db, 0x119e684de476155f], [0xcff784b97b3fd800, 0xb51248c23828f047, 0x188bea59ae363537, 0x1835b786e2e8925e]],
    // Round 40
    [[0x6c40e285ab32eeb6, 0xd152bac2a7905c92, 0x4d794996c6433a20, 0x28201a34c594dfa3], [0x4a761f88c22cc4e7, 0x864c82eb57118772, 0x94e80fefaf78b000, 0x083efd7a27d17510], [0x9e079564f61fd13b, 0x11c16df7774dd851, 0x6158e61ceea27be8, 0x0b6f88a357719952]],
    // Round 41
    [[0x14390e6ee4254f5b, 0x589511ca00d29e10, 0x644f66e1d6471a94, 0x0ec868e6d15e51d9], [0x00d937ab84c98591, 0xecd3e74b939cd40d, 0x1ac0c9b3ed2e1142, 0x2af33e3f86677127], [0x364ce5e47951f178, 0x34568c547dd6858b, 0xd09b5d961c6ace77, 0x0b520211f904b5e7]],
    // Round 42
    [[0xca228620188a1d40, 0xa0c56ac4270e822c, 0xd8db58f10062a92e, 0x0b2d722d0919a1aa], [0xe0061d1ed6e562d4, 0x57b54a9991ca38bb, 0xd980ceb37c2453e9, 0x1f790d4d7f8cf094], [0xda92ceb01e504233, 0x0885c16235a2a6a8, 0xaea97cd385f78015, 0x0171eb95dfbf7d1e]],
    // Round 43
    [[0x762305381b168873, 0x790b40defd2c8650, 0x329bf6885da66b9b, 0x0c2d0e3b5fd57549], [0x5d3803054407a18d, 0x7cbcafa589e283c3, 0x4e5a8228b4e72b37, 0x1162fb28689c2715], [0x1623ef8249711bc0, 0x282c5a92a89e1992, 0x64ad386a91e8310f, 0x2f1459b65dee441b]],
    // Round 44
    [[0xc243f70d1b53cfbb, 0xbc489d46754eb712, 0x996d74367d5cd4c1, 0x1e6ff3216b688c3d], [0x76881f9326478875, 0xd741a6f36cdc2a05, 0x681487d27d157802, 0x01ca8be73832b8d0], [0x0b9b5de315f9650e, 0x680286080b10cea0, 0x86f976d5bdf223dc, 0x1f7735706ffe9fc5]],
    // Round 45
    [[0x4745ca838285f019, 0x21ac10a3d5f096ef, 0x40a0c2dce041fba9, 0x2522b60f4ea33076], [0x8ce16c235572575b, 0x3418cad4f52b6c3f, 0x5255075ddc957f83, 0x23f0bee001b1029d], [0x66d9401093082d59, 0x5d142633e9df905f, 0xcaac2d44555ed568, 0x2bc1ae8b8ddbb81f]],
    // Round 46
    [[0x8011fcd6ad72205f, 0x62371273a07b1fc9, 0x7304507b8dba3ed1, 0x0f9406b8296564a3], [0xcb126c8cd995f0a8, 0x17e75b174a52ee4a, 0x67b72998de90714e, 0x2360a8eb0cc7defa], [0x6dcbbc2767f88948, 0xb4815a5e96df8b00, 0x804c803cbaef255e, 0x15871a5cddead976]],
    // Round 47
    [[0x4f957ccdeefb420f, 0x362f4f54f7237954, 0x0a8652dd2f3b1da0, 0x193a56766998ee9e], [0xe4309805e777ae0f, 0x3b2e63c8ad334834, 0x2f9be56ff4fab170, 0x2a394a43934f8698], [0xb4166e8876c0d142, 0x892cd11223443ba7, 0x3e8b635dcb345192, 0x1859954cfeb8695f]],
    // Round 48
    [[0x408d3819f4fed32b, 0x2b11bc25d90bbdca, 0x013444dbcb99f190, 0x04e1181763050e58], [0x1f5e5552bfd05f23, 0xb10eb82db08b5e8b, 0x40c335ea64de8c5b, 0x0fdb253dee83869d], [0xa9d7c5bae9b4f1c0, 0x75f08686f1c08984, 0xaa4efb623adead62, 0x058cbe8a9a5027bd]],
    // Round 49
    [[0xd15228b4cceca59a, 0x23b4b83bef023ab0, 0x497eadb1aeb1f52b, 0x1382edce9971e186], [0xe1e6634601d9e8b5, 0x7f61b8eb99f14b77, 0x0819ca51fd11b0be, 0x03464990f045c6ee], [0xaa5bc137aeb70a58, 0x6fcab4605db2eb5a, 0xfff33b41f98ff83c, 0x23f7bfc8720dc296]],
    // Round 50
    [[0x19636158bbaf62f2, 0x18c3ffd5e1531a92, 0x7e6e94e7f0e9decf, 0x0a59a158e3eec211], [0xf4c23ed0075fd07b, 0xe2c4eba065420af8, 0xb58bf23b312ffd3c, 0x06ec54c80381c052], [0x962f0ff9ed1f9d01, 0xb09340f7a7bcb1b4, 0x476b56648e867ec8, 0x118872dc832e0eb5]],
    // Round 51
    [[0x95e1906b520921b1, 0x52e0b0f0e42d7fea, 0x5ad5c7cba7ad59ed, 0x13d69fa127d83416], [0xfd8a49f19f10c77b, 0xde143942fb71dc55, 0x70b1c6877a73d21b, 0x169a177f63ea6812], [0xfb7e9a5a7450544d, 0x3abeb032b922f66f, 0xef42f287adce40d9, 0x04ef51591c6ead97]],
    // Round 52
    [[0xd5f45ee6dd0f69ec, 0x19ec61805d4f03ce, 0x0ecd7ca703fb2e3b, 0x256e175a1dc07939], [0xa002813d3e2ceeb2, 0x75cc360d3205dd2d, 0xe5f2af412ff6004f, 0x30102d28636abd5f], [0x1fd31be182fcc792, 0x0443a3fa99bef4a3, 0x1c0714bc73eb1bf4, 0x10998e42dfcd3bbf]],
    // Round 53
    [[0xecad76f879e36860, 0x9f3362eaf4d582ef, 0x25fa7d24b598a1d8, 0x193edd8e9fcf3d76], [0xf2664d7aa51f0b5d, 0xd1c7a561ce611425, 0xd0368ce80b7b3347, 0x18168afd34f2d915], [0x29e2e95b33ea6111, 0xa328ec77bc33626e, 0x0c017656ebe658b6, 0x29383c01ebd3b6ab]],
    // Round 54
    [[0x00bf573f9010c711, 0x702db6e86fb76ab6, 0xa1f4ae5e7771a64a, 0x10646d2f2603de39], [0x64d0242dcb1117fb, 0x2f90c25b40da7b38, 0xf575f1395a55bf13, 0x0beb5e07d1b27145], [0xdffbf018d96fa336, 0x30f95bb2e54b59ab, 0xdc0d3ecad62b5c88, 0x16d685252078c133]],
    // Round 55
    [[0xfd672dd62047f01a, 0x0a555bbbec21ddfa, 0x3c74154e0404b4b4, 0x0a6abd1d833938f3], [0x70a6f19b34cf1860, 0xb12dffeec4503172, 0x8ea12a4c2dedc8fe, 0x1a679f5d36eb7b5c], [0xfbc7592e3f1b93d6, 0x26a423eada4e8f6f, 0x3974d50e0ebfde47, 0x0980fb233bd456c2]],
    // Round 56
    [[0x03ebacb5c312c72b, 0xcece3d5628c92820, 0xbf1810af93a38fc0, 0x161b42232e61b84c], [0xd09203db47de1a0b, 0x493f09787f1564e5, 0x950f7d47a60d5e6a, 0x0ada10a90c7f0520], [0xb50ddb9af407f451, 0xd3f07a8a2b4e121b, 0x320345a29ac4238e, 0x1a730d372310ba82]],
    // Round 57
    [[0xfbda10ef58e8c556, 0x908377feaba5c4df, 0x817064c369dda7ea, 0x2c8120f268ef054f], [0x6e7b8649a4968f70, 0xb930e95313bcb73e, 0xa57c00789c684217, 0x1c7c8824f758753f], [0xb47b27fa3fd1cf77, 0xf400ad8b491eb3f7, 0x8e39e4077a74faa0, 0x2cd9ed31f5f8691c]],
    // Round 58
    [[0x854ae23918a22eea, 0xa5e022ac321ca550, 0xcf60d92f57618399, 0x23ff4f9d46813457], [0xdff1ea58f180426d, 0xaf5a2c5103529407, 0xceece6405dddd9d0, 0x09945a5d147a4f66], [0x8a6dd223ec6fc630, 0x7c7da6eaa29d3f26, 0xb67660c6b771b90f, 0x188d9c528025d4c2]],
    // Round 59
    [[0xe0c0d8ddf4f0f47f, 0xdba7d926d3633595, 0x81f68311431d8734, 0x3050e37996596b7f], [0x9d829518d30afd78, 0x6ceae5461e3f95d8, 0x1600ca8102c35c42, 0x15af1169396830a9], [0x04284da3320d8acc, 0xdae933e351466b29, 0xa06d9f37f873d985, 0x1da6d09885432ea9]],
    // Round 60
    [[0xe546ee411ddaa9cb, 0x4e4fad3dbe658945, 0xf5f8acf33921124e, 0x2796ea90d269af29], [0x7cb0319e01d32d60, 0x1e15612ec8e9304a, 0x0325c8b3307742f0, 0x202d7dd1da0f6b4b], [0xa29dace4c0f8be5f, 0xa2d7f9c788f4c831, 0x156a952ba263d672, 0x096d6790d05bb759]],
    // Round 61
    [[0x63798cb1447d25a4, 0x438da23ce5b13e19, 0x83808965275d877b, 0x054efa1f65b0fce2], [0x64ccf6e18e4165f1, 0xd8aa690113b2e148, 0xdb3308c29802deb9, 0x1b162f83d917e93e], [0xc5ceb745a0506edc, 0xedfefc1466cc568e, 0xfd9f1cdd2a0de39e, 0x21e5241e12564dd6]],
    // Round 62
    [[0x7b4349e10e4bdf08, 0xcb73ab5f87e16192, 0x226a80ee17b36abe, 0x1cfb5662e8cf5ac9], [0x29c53f666eb24100, 0x2c99af346220ac01, 0xbae6d8d1ecb373b6, 0x0f21177e302a771b], [0xbcef7e1f515c2320, 0xc4236aede6290546, 0xaffb0dd7f71b12be, 0x1671522374606992]],
    // Round 63
    [[0xd419d2a692cad870, 0xbe2ec9e42c5cc8cc, 0x2eb4cf24501bfad9, 0x0fa3ec5b9488259c], [0x85e8c57b1ab54bba, 0xd36edce85c648cc0, 0x57cb266c1506080e, 0x193c0e04e0bd2983], [0xce14ea2adaba68f8, 0x9f6f7291cd406578, 0x7e9128306dcbc3c9, 0x102adf8ef74735a2]],
    // Round 64
    [[0x40a6d0cb70c3eab1, 0x316aa24bfbdd23ae, 0xe2a54d6f1ad945b1, 0x0fe0af7858e49859], [0xe8a5ea7344798d22, 0x2da5f1daa9ebdefd, 0x08536a2220843f4e, 0x216f6717bbc7dedb], [0xf88e2e4228325161, 0x3c23b2ac773c6b3e, 0x4a3e694391918a1b, 0x1da55cc900f0d21f]],
];

/// Convert raw limbs to `Fq`.
#[inline(always)]
fn rc(limbs: &[u64; 4]) -> Fq {
    Fq(U256(*limbs))
}

/// MDS matrix (3x3) for the linear layer.
/// Extracted from PoseidonSponge.sol assembly.
#[rustfmt::skip]
const MDS: [[[u64; 4]; 3]; 3] = [
    [[0xcaaae5c261530d87, 0x2060343985897776, 0xa6c27556afb5d04a, 0x2902033cc64dfdd0], [0xa3bdcfb6e50f8518, 0xbd8afe14a805fc6d, 0x5d0126079fa8fb0f, 0x0c8e05cc1dce3132], [0xea005b0477b76195, 0x46d9f747b40ab013, 0x5f7cf18d72818c2e, 0x2ca6dad73bbb9dca]],
    [[0xd13cd31103b51b6d, 0xf7fd17073b3744b2, 0x9b2a5d893de9d0ab, 0x1de80ae0a294a54f], [0x9e9f9f644c7f4c28, 0x308e20fbee4555a4, 0x724a8cc653cccaf9, 0x06b19eaaf1638636], [0xe1a4ad81749146cc, 0xc59cfc7224930625, 0xf887d65926e64647, 0x1b10b07bf0544e3d]],
    [[0x339a9d059f928b0e, 0xa842d88af7bd3436, 0x1be9d8955442bff7, 0x2978b3b1e42d4679], [0x7756806c4006d434, 0x9ff0a6b9f88a6816, 0xd9fb8df22f8f2e40, 0x175b4a96ae4a9a7f], [0x72fddea1067f9c3f, 0x96b60333f2000ef7, 0xcb7284890d5a6a8e, 0x16667380ea38a6b4]],
];

/// Poseidon sponge state.
///
/// Width 3, rate 2. The capacity element is `state[0]`;
/// rate positions are `state[1]` and `state[2]`.
pub struct PoseidonSponge {
    state: [Fq; 3],
    /// Number of elements absorbed since the last permutation (0 or 1).
    absorbing: usize,
}

impl PoseidonSponge {
    /// Create a new sponge with the standard domain separator.
    ///
    /// Initial state: `[2^64, 0, 0]` matching PoseidonSponge.sol `init()`.
    pub fn init() -> Self {
        // 2^64 = U256([0, 1, 0, 0]) because limb[0] holds bits 0..63
        // and limb[1] holds bits 64..127.
        PoseidonSponge {
            state: [Fq(U256([0, 1, 0, 0])), Fq::ZERO, Fq::ZERO],
            absorbing: 0,
        }
    }

    /// Absorb a single field element into the sponge.
    ///
    /// Adds the value to the next rate position. When both rate positions
    /// are filled (after absorbing two elements), the sponge permutes
    /// automatically.
    pub fn absorb(&mut self, value: &Fq) {
        // Rate positions are state[1] and state[2].
        // absorbing==0 targets state[1], absorbing==1 targets state[2].
        self.state[self.absorbing + 1] = self.state[self.absorbing + 1].add(value);
        self.absorbing += 1;
        if self.absorbing == 2 {
            self.permute();
            self.absorbing = 0;
        }
    }

    /// Absorb a raw U256 value (interpreted as an Fq element).
    pub fn absorb_u256(&mut self, value: &U256) {
        self.absorb(&Fq::new(*value));
    }

    /// Squeeze a single challenge from the sponge.
    ///
    /// Pads by adding 1 to the next rate position, permutes, resets the
    /// absorb counter, and returns `state[1]`.
    pub fn squeeze(&mut self) -> Fq {
        // Pad: add 1 to the next rate position.
        self.state[self.absorbing + 1] = self.state[self.absorbing + 1].add(&Fq::ONE);
        self.permute();
        self.absorbing = 0;
        self.state[1]
    }

    /// Full Poseidon permutation (65 rounds).
    ///
    /// Rounds 0..3: full rounds (S-box on all 3 state elements)
    /// Rounds 4..60: partial rounds (S-box on state[0] only)
    /// Rounds 61..64: full rounds (S-box on all 3 state elements)
    fn permute(&mut self) {
        for (round, round_constants) in ROUND_CONSTANTS.iter().enumerate() {
            // --- Add round constants ---
            self.state[0] = self.state[0].add(&rc(&round_constants[0]));
            self.state[1] = self.state[1].add(&rc(&round_constants[1]));
            self.state[2] = self.state[2].add(&rc(&round_constants[2]));

            // --- S-box (x^5) ---
            if !(FULL_FIRST..FULL_LAST_START).contains(&round) {
                // Full round: apply S-box to all state elements.
                self.state[0] = self.state[0].pow5();
                self.state[1] = self.state[1].pow5();
                self.state[2] = self.state[2].pow5();
            } else {
                // Partial round: apply S-box only to state[0].
                self.state[0] = self.state[0].pow5();
            }

            // --- MDS matrix multiplication ---
            let s0 = self.state[0];
            let s1 = self.state[1];
            let s2 = self.state[2];
            self.state[0] = rc(&MDS[0][0])
                .mul(&s0)
                .add(&rc(&MDS[0][1]).mul(&s1))
                .add(&rc(&MDS[0][2]).mul(&s2));
            self.state[1] = rc(&MDS[1][0])
                .mul(&s0)
                .add(&rc(&MDS[1][1]).mul(&s1))
                .add(&rc(&MDS[1][2]).mul(&s2));
            self.state[2] = rc(&MDS[2][0])
                .mul(&s0)
                .add(&rc(&MDS[2][1]).mul(&s1))
                .add(&rc(&MDS[2][2]).mul(&s2));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_state() {
        let s = PoseidonSponge::init();
        // 2^64 in U256 LE limbs
        assert_eq!(s.state[0], Fq(U256([0, 1, 0, 0])));
        assert_eq!(s.state[1], Fq::ZERO);
        assert_eq!(s.state[2], Fq::ZERO);
        assert_eq!(s.absorbing, 0);
    }

    #[test]
    fn test_absorb_one_element() {
        let mut s = PoseidonSponge::init();
        s.absorb(&Fq::from_u64(1));
        // After one absorb, absorbing counter should be 1
        assert_eq!(s.absorbing, 1);
        // state[1] should have the value added
        assert_eq!(s.state[1], Fq::from_u64(1));
        // state[0] (capacity) unchanged at 2^64
        assert_eq!(s.state[0], Fq(U256([0, 1, 0, 0])));
    }

    #[test]
    fn test_absorb_triggers_permute() {
        let mut s = PoseidonSponge::init();
        s.absorb(&Fq::from_u64(1));
        assert_eq!(s.absorbing, 1);
        s.absorb(&Fq::from_u64(2));
        // After two absorbs, permutation has been applied and counter reset
        assert_eq!(s.absorbing, 0);
        // State should have been permuted (no longer the initial values)
        assert_ne!(s.state[0], Fq(U256([0, 1, 0, 0])));
    }

    #[test]
    fn test_squeeze_deterministic() {
        let mut s1 = PoseidonSponge::init();
        let mut s2 = PoseidonSponge::init();
        s1.absorb(&Fq::from_u64(42));
        s2.absorb(&Fq::from_u64(42));
        let r1 = s1.squeeze();
        let r2 = s2.squeeze();
        assert_eq!(r1, r2);
    }

    #[test]
    fn test_squeeze_different_inputs() {
        let mut s1 = PoseidonSponge::init();
        let mut s2 = PoseidonSponge::init();
        s1.absorb(&Fq::from_u64(1));
        s2.absorb(&Fq::from_u64(2));
        let r1 = s1.squeeze();
        let r2 = s2.squeeze();
        assert_ne!(r1, r2);
    }

    #[test]
    fn test_squeeze_resets_absorbing() {
        let mut s = PoseidonSponge::init();
        s.absorb(&Fq::from_u64(100));
        assert_eq!(s.absorbing, 1);
        let _ = s.squeeze();
        assert_eq!(s.absorbing, 0);
    }

    #[test]
    fn test_absorb_u256() {
        let mut s1 = PoseidonSponge::init();
        let mut s2 = PoseidonSponge::init();
        let val = U256::from_u64(999);
        s1.absorb_u256(&val);
        s2.absorb(&Fq::from_u64(999));
        // Both should produce the same sponge state
        assert_eq!(s1.squeeze(), s2.squeeze());
    }

    #[test]
    fn test_constants_nonzero() {
        // Every round constant should be non-zero (all PSE/Scroll constants are)
        for (round, round_constants) in ROUND_CONSTANTS.iter().enumerate() {
            for (j, constant) in round_constants.iter().enumerate() {
                assert_ne!(
                    rc(constant),
                    Fq::ZERO,
                    "Round constant [{round}][{j}] should not be zero"
                );
            }
        }
    }

    #[test]
    fn test_mds_nonzero() {
        for i in 0..3 {
            for j in 0..3 {
                assert_ne!(rc(&MDS[i][j]), Fq::ZERO, "MDS[{i}][{j}] should not be zero");
            }
        }
    }

    #[test]
    fn test_multiple_squeeze() {
        // Squeezing multiple times should produce different values
        let mut s = PoseidonSponge::init();
        s.absorb(&Fq::from_u64(42));
        let r1 = s.squeeze();
        let r2 = s.squeeze();
        assert_ne!(r1, r2);
    }

    #[test]
    fn test_absorb_squeeze_absorb_squeeze() {
        // Interleaved absorb/squeeze should be deterministic
        let mut s1 = PoseidonSponge::init();
        let mut s2 = PoseidonSponge::init();

        s1.absorb(&Fq::from_u64(10));
        s2.absorb(&Fq::from_u64(10));
        let r1a = s1.squeeze();
        let r2a = s2.squeeze();
        assert_eq!(r1a, r2a);

        s1.absorb(&Fq::from_u64(20));
        s2.absorb(&Fq::from_u64(20));
        let r1b = s1.squeeze();
        let r2b = s2.squeeze();
        assert_eq!(r1b, r2b);
        assert_ne!(r1a, r1b);
    }

    #[test]
    fn test_squeeze_no_absorb() {
        // Squeezing from a fresh sponge (no absorb) should work
        let mut s = PoseidonSponge::init();
        let r = s.squeeze();
        // Result should be non-zero (permutation of [2^64, 1, 0] after padding)
        assert_ne!(r, Fq::ZERO);
    }

    #[test]
    fn test_round0_constant_matches_solidity() {
        // Verify round 0 c0 matches the hex from PoseidonSponge.sol:
        // 0x0ee9a592ba9a9518d05986d656f40c2114c4993c11bb29938d21d47304cd8e6e
        let expected = Fq(U256([
            0x8d21d47304cd8e6e,
            0x14c4993c11bb2993,
            0xd05986d656f40c21,
            0x0ee9a592ba9a9518,
        ]));
        assert_eq!(rc(&ROUND_CONSTANTS[0][0]), expected);
    }

    #[test]
    fn test_mds00_matches_solidity() {
        // Verify MDS[0][0] matches the hex from PoseidonSponge.sol:
        // 0x2902033cc64dfdd0a6c27556afb5d04a2060343985897776caaae5c261530d87
        let expected = Fq(U256([
            0xcaaae5c261530d87,
            0x2060343985897776,
            0xa6c27556afb5d04a,
            0x2902033cc64dfdd0,
        ]));
        assert_eq!(rc(&MDS[0][0]), expected);
    }

    // ============================================================
    // Cross-check tests: compare Rust outputs to Solidity PoseidonSponge.sol
    // Reference values obtained from PoseidonCrossCheckTest.t.sol
    // ============================================================

    #[test]
    fn test_cross_squeeze_no_absorb() {
        let mut s = PoseidonSponge::init();
        let result = s.squeeze();
        let expected = Fq(U256([
            0x78532d0508f5d439,
            0xe11c70de9b5978ae,
            0xdb0810e362cfd943,
            0x140230d79a04f0bb,
        ]));
        assert_eq!(result, expected, "squeeze_no_absorb mismatch with Solidity");
    }

    #[test]
    fn test_cross_absorb_1_squeeze() {
        let mut s = PoseidonSponge::init();
        s.absorb(&Fq::from_u64(1));
        let result = s.squeeze();
        let expected = Fq(U256([
            0xd28fee1a8ebdd9dd,
            0x6e1f08976f455c78,
            0xa0565c77d56d22b0,
            0x11b59b2a25b09e83,
        ]));
        assert_eq!(result, expected, "absorb(1)+squeeze mismatch with Solidity");
    }

    #[test]
    fn test_cross_absorb_42_squeeze() {
        let mut s = PoseidonSponge::init();
        s.absorb(&Fq::from_u64(42));
        let result = s.squeeze();
        let expected = Fq(U256([
            0x830c63e4318c8f38,
            0x9f26c1c9f4bccc65,
            0x20a1fc2e0bbbce8d,
            0x2294045aa6c16b19,
        ]));
        assert_eq!(
            result, expected,
            "absorb(42)+squeeze mismatch with Solidity"
        );
    }

    #[test]
    fn test_cross_absorb_1_2_squeeze() {
        let mut s = PoseidonSponge::init();
        s.absorb(&Fq::from_u64(1));
        s.absorb(&Fq::from_u64(2));
        let result = s.squeeze();
        let expected = Fq(U256([
            0xddf98e89273d4d80,
            0x5e761689eef69952,
            0x6d2e8b47816ae6f2,
            0x2f09cecd4c83e5fb,
        ]));
        assert_eq!(
            result, expected,
            "absorb(1,2)+squeeze mismatch with Solidity"
        );
    }

    #[test]
    fn test_cross_absorb_1_2_3_squeeze() {
        let mut s = PoseidonSponge::init();
        s.absorb(&Fq::from_u64(1));
        s.absorb(&Fq::from_u64(2));
        s.absorb(&Fq::from_u64(3));
        let result = s.squeeze();
        let expected = Fq(U256([
            0x6e832396d96c3206,
            0x2f79a152b259cb83,
            0xfd9fe08c6dfb1a3a,
            0x15fbbb5538ed972f,
        ]));
        assert_eq!(
            result, expected,
            "absorb(1,2,3)+squeeze mismatch with Solidity"
        );
    }

    #[test]
    fn test_cross_absorb_large_squeeze() {
        let mut s = PoseidonSponge::init();
        // Fq modulus - 1
        let large = Fq(U256([
            0x3c208c16d87cfd46,
            0x97816a916871ca8d,
            0xb85045b68181585d,
            0x30644e72e131a029,
        ]));
        s.absorb(&large);
        let result = s.squeeze();
        let expected = Fq(U256([
            0x9d4c47dc93844892,
            0xf95fd3e58a654fcf,
            0xd2d4100f78cdf4af,
            0x2e6a55fb23f60b6a,
        ]));
        assert_eq!(
            result, expected,
            "absorb(large)+squeeze mismatch with Solidity"
        );
    }

    #[test]
    fn test_cross_absorb_pair_squeeze() {
        let mut s = PoseidonSponge::init();
        s.absorb(&Fq::from_u64(100));
        s.absorb(&Fq::from_u64(200));
        let result = s.squeeze();
        let expected = Fq(U256([
            0x189d7c1b13523cf2,
            0xf67e12b4cbfd1e67,
            0xac5e44a991b2d9e5,
            0x0c981642bd2f9e41,
        ]));
        assert_eq!(
            result, expected,
            "absorb_pair(100,200)+squeeze mismatch with Solidity"
        );
    }

    #[test]
    fn test_cross_double_squeeze() {
        let mut s = PoseidonSponge::init();
        s.absorb(&Fq::from_u64(99));
        let r1 = s.squeeze();
        let r2 = s.squeeze();
        let expected1 = Fq(U256([
            0x2ab0a6f647c2d87c,
            0x318b5171cbee83c2,
            0x5b7bf67706ef11bd,
            0x12c2ed9eacfae74b,
        ]));
        let expected2 = Fq(U256([
            0x58b457c19d51aeb9,
            0x913a5408ceb7e9e3,
            0x69a9b23b09987ab6,
            0x2d09ccf58cfb749f,
        ]));
        assert_eq!(r1, expected1, "double_squeeze r1 mismatch with Solidity");
        assert_eq!(r2, expected2, "double_squeeze r2 mismatch with Solidity");
    }
}
