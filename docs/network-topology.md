# AWEP2P — ամբողջական Node-to-Node topology

## Ցանցի կառուցվածքը

**Node → Node → Data Centre → Data Group → Centre Group → AWE Net**

### Node → Node
Յուրաքանչյուր node պահում է իր peer-երի ցանկը։ Նոր node-ը նախ միանում է հասանելի ամենամոտ/հարմար peer-ին։ Կապերը երկկողմ են, իսկ երթուղիները կարող են անցնել մի քանի node-ով։

### Data Centre
Data Centre-ի ներսում կիրառվում է Full Mesh․ առողջ node-երը կարող են անմիջապես կապվել միմյանց հետ։

### Data Centre → Data Centre
Աջակցվում է երկու ձև.
1. **Full Mesh:** առաջին DC-ի բոլոր node-երը կապվում են երկրորդ DC-ի բոլոր node-երին։
2. **Relay:** առաջին DC-ի ընտրված relay node-ը կապ է ստեղծում երկրորդ DC-ի node-ի հետ։

### Data Group
**3 կամ ավելի Data Centre = Data Group**։

### Centre Group
**2 կամ ավելի Data Group = Centre Group**։

### AWE Net
**Բոլոր Centre Group-երը միասին = AWE Net**։

### Ուղղորդում և խափանումների դիմադրություն
Routing-ը աշխատում է ընթացիկ peer graph-ի վրա՝ BFS-ով։ Կենտրոնական routing server պարտադիր չէ։ Full Mesh-ի դեպքում մի ուղու խափանումը կարող է թողնել այլ ուղիներ։

> Իրական production կապի համար topology layer-ը պետք է միացվի անվտանգ transport/discovery/heartbeat շերտերին։ Այս մոդուլը այդ topology-ի state և routing հիմքն է։